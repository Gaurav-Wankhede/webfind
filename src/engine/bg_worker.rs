use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::embedder::Embedder;
use crate::schema::content::{PageContentRecord, StructuredContent};

/// Job sent to the background worker to persist full page content + embeddings.
#[derive(Debug, Clone)]
pub struct PersistJob {
    pub url: String,
    pub content: StructuredContent,
}

/// Background worker that receives fetched page content over a channel and
/// persists it into the knowledge graph (`page_content`) and vector DB
/// (`url_node.embedding`) without blocking the request path.
pub struct BackgroundWorker {
    sender: Mutex<Option<mpsc::Sender<PersistJob>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl BackgroundWorker {
    /// Spawn a worker with the given graph store and embedder.
    ///
    /// `capacity` controls the in-memory channel back-pressure. If the worker
    /// falls behind, senders will block until space is available.
    pub fn new(
        graph_store: Arc<dyn CrawlGraphStore + Send + Sync>,
        embedder: Option<Arc<dyn Embedder>>,
        capacity: usize,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel::<PersistJob>(capacity);

        let handle = tokio::spawn(async move {
            while let Some(job) = receiver.recv().await {
                if let Err(e) = process_job(&graph_store, embedder.clone(), job).await {
                    tracing::error!("background persist job failed: {}", e);
                }
            }
            tracing::info!("background worker channel closed, shutting down");
        });

        Self {
            sender: Mutex::new(Some(sender)),
            handle: Mutex::new(Some(handle)),
        }
    }

    /// Enqueue a page for background persistence.
    pub async fn persist(&self, url: String, content: StructuredContent) -> Result<()> {
        let sender = self
            .sender
            .lock()
            .await
            .as_ref()
            .context("background worker is closed")?
            .clone();
        sender
            .send(PersistJob { url, content })
            .await
            .context("background worker channel closed")
    }

    /// Close the channel and wait for all queued jobs to finish processing.
    pub async fn close(&self) {
        let sender = self.sender.lock().await.take();
        drop(sender);
        if let Some(handle) = self.handle.lock().await.take() {
            let _ = handle.await;
        }
    }
}

async fn process_job(
    graph_store: &Arc<dyn CrawlGraphStore + Send + Sync>,
    embedder: Option<Arc<dyn Embedder>>,
    job: PersistJob,
) -> Result<()> {
    let url_node_id = format!("url_node:{}", url_to_id(&job.url));
    let record = PageContentRecord::from_content(&job.content, url_node_id);

    // Track durable job status for observability/queue semantics.
    let job_id = graph_store
        .enqueue_crawl_job(&job.url)
        .await
        .unwrap_or_else(|_| url_to_id(&job.url));
    let _ = graph_store
        .mark_crawl_job_status(&job_id, "processing", None)
        .await;

    // Persist full content into the knowledge graph.
    if let Err(e) = graph_store.record_page_content(record).await {
        let _ = graph_store
            .mark_crawl_job_status(&job_id, "failed", Some(&e.to_string()))
            .await;
        return Err(e);
    }

    // Generate and persist embedding in a blocking task to avoid starving
    // the async runtime with ONNX/FastEmbed work.
    if let Some(embedder) = embedder {
        let text = format!(
            "{} {} {}",
            job.content.title, job.content.excerpt, job.content.content_text
        );
        let text = text.trim().to_string();
        if !text.is_empty() {
            let url = job.url.clone();
            let graph_store = graph_store.clone();
            let job_id = job_id.clone();
            let handle = tokio::task::spawn_blocking(move || embedder.embed(&[&text]));
            match handle.await {
                Ok(Ok(vectors)) if !vectors.is_empty() => {
                    if let Err(e) = graph_store.record_embedding(&url, vectors[0].clone()).await {
                        let _ = graph_store
                            .mark_crawl_job_status(&job_id, "failed", Some(&e.to_string()))
                            .await;
                        return Err(e);
                    }
                }
                Ok(Ok(_)) => {
                    let msg = "embedding produced no vectors";
                    let _ = graph_store
                        .mark_crawl_job_status(&job_id, "failed", Some(msg))
                        .await;
                }
                Ok(Err(e)) => {
                    let msg = format!("embedding failed: {}", e);
                    let _ = graph_store
                        .mark_crawl_job_status(&job_id, "failed", Some(&msg))
                        .await;
                    return Err(e);
                }
                Err(e) => {
                    let msg = format!("embedding task panicked: {}", e);
                    let _ = graph_store
                        .mark_crawl_job_status(&job_id, "failed", Some(&msg))
                        .await;
                    return Err(anyhow::anyhow!(msg));
                }
            }
        }
    }

    graph_store
        .mark_crawl_job_status(&job_id, "done", None)
        .await?;
    Ok(())
}

/// Stable URL-derived identifier using BLAKE3.
/// Delegates to the shared `url_id()` utility in `engine::util`.
fn url_to_id(url: &str) -> String {
    crate::engine::util::url_id(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::crawl_graph::InMemoryCrawlGraph;
    use chrono::Utc;

    fn sample_content(url: &str) -> StructuredContent {
        StructuredContent {
            url: url.to_string(),
            final_url: url.to_string(),
            status_code: 200,
            title: "Test".to_string(),
            description: None,
            canonical_url: None,
            language: "en".to_string(),
            language_confidence: 0.95,
            published_at: None,
            modified_at: None,
            author: None,
            site_name: None,
            content_text: "This is test content.".to_string(),
            content_html: "<p>This is test content.</p>".to_string(),
            content_markdown: "This is test content.".to_string(),
            excerpt: "This is test content.".to_string(),
            word_count: 5,
            char_count: 21,
            sentence_count: 1,
            reading_time_seconds: 1,
            reading_ease: 70.0,
            grade_level: 8.0,
            keywords: vec![],
            open_graph: None,
            twitter_card: None,
            json_ld: vec![],
            schema_type: None,
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: "This is test content.".to_string(),
            fetched_at: Utc::now(),
            fetch_duration_ms: 100,
            html_size_bytes: 100,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            is_paywalled: false,
            is_valid_content: true,
            content_type: "text/html".to_string(),
            content_type_header: "text/html".to_string(),
            entities: crate::schema::content::Entities::default(),
        }
    }

    #[tokio::test]
    async fn test_background_worker_persists_job() {
        let graph = Arc::new(InMemoryCrawlGraph::new());
        let worker = BackgroundWorker::new(graph.clone(), None, 16);

        let url = "https://example.com/page".to_string();
        worker
            .persist(url.clone(), sample_content(&url))
            .await
            .unwrap();
        worker.close().await;

        // When processing finishes, no pending jobs remain.
        let jobs = graph.dequeue_crawl_jobs(10).await.unwrap();
        assert!(jobs.is_empty());
    }
}
