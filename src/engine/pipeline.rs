use std::time::Duration;

use futures::future::join_all;

use super::fetcher::Fetcher;
use crate::schema::content::StructuredContent;

/// Result of one URL fetched through the parallel pipeline.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub url: String,
    pub success: bool,
    pub content: Option<StructuredContent>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Fetches multiple URLs in parallel and returns clean structured results.
pub struct FetchPipeline {
    fetcher: Fetcher,
    concurrency: usize,
    timeout_ms: u64,
}

impl FetchPipeline {
    pub fn new(fetcher: Fetcher) -> Self {
        Self {
            fetcher,
            concurrency: 8,
            timeout_ms: 30_000,
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms.max(1_000);
        self
    }

    /// Fetch all URLs in parallel with bounded concurrency.
    ///
    /// This is the async parallel stage. Downstream consumers receive a Vec of
    /// clean `PipelineResult`s and can process them synchronously.
    pub async fn fetch_all(&self, urls: &[String]) -> Vec<PipelineResult> {
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(self.concurrency));
        let fetcher = &self.fetcher;

        let mut tasks = Vec::with_capacity(urls.len());
        for url in urls.iter().cloned() {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let timeout = Duration::from_millis(self.timeout_ms);
            let fut = async move {
                let start = std::time::Instant::now();
                let result = tokio::time::timeout(timeout, fetcher.fetch_url(&url)).await;
                drop(permit);
                let duration_ms = start.elapsed().as_millis() as u64;

                match result {
                    Ok(Ok(content)) => PipelineResult {
                        url,
                        success: true,
                        content: Some(content),
                        error: None,
                        duration_ms,
                    },
                    Ok(Err(e)) => PipelineResult {
                        url,
                        success: false,
                        content: None,
                        error: Some(e.to_string()),
                        duration_ms,
                    },
                    Err(_) => PipelineResult {
                        url,
                        success: false,
                        content: None,
                        error: Some("timeout".to_string()),
                        duration_ms,
                    },
                }
            };
            tasks.push(fut);
        }

        join_all(tasks).await
    }

    /// Synchronous cleanup/filter stage: keep only successful, valid content.
    pub fn filter_valid(results: Vec<PipelineResult>) -> Vec<StructuredContent> {
        results
            .into_iter()
            .filter_map(|r| r.content)
            .filter(|c| c.is_valid_content)
            .collect()
    }

    /// Synchronous summary stage: returns (success_count, failure_count, errors).
    pub fn summarize(results: &[PipelineResult]) -> (usize, usize, Vec<(String, String)>) {
        let success = results.iter().filter(|r| r.success).count();
        let failure = results.len() - success;
        let errors: Vec<_> = results
            .iter()
            .filter(|r| !r.success)
            .map(|r| (r.url.clone(), r.error.clone().unwrap_or_default()))
            .collect();
        (success, failure, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize() {
        let results = vec![
            PipelineResult {
                url: "https://a".to_string(),
                success: true,
                content: None,
                error: None,
                duration_ms: 10,
            },
            PipelineResult {
                url: "https://b".to_string(),
                success: false,
                content: None,
                error: Some("fail".to_string()),
                duration_ms: 20,
            },
        ];
        let (s, f, e) = FetchPipeline::summarize(&results);
        assert_eq!(s, 1);
        assert_eq!(f, 1);
        assert_eq!(e.len(), 1);
    }
}
