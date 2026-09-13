use webfind::engine::device_profile::{SessionManager, StickySessions};
use webfind::engine::fetcher::{Fetcher, RotateUserAgent};
use webfind::engine::proxy_pool::ProxyPool;
use webfind::engine::util::split_comma;
use webfind::render::{BoxFormat, MarkdownFormat, RenderContext, RenderFormat};
use webfind::schema::content::StructuredContent;
use webfind::schema::request::OutputFormat;

fn print_fetch_report(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    let ctx = RenderContext::from_content(content, content.excerpt.clone());
    let renderer = BoxFormat;
    let output = renderer.render(&ctx, show_links, show_keywords);
    print!("{}", output);
}

fn print_fetch_markdown(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    let ctx = RenderContext::from_content(content, content.excerpt.clone());
    let renderer = MarkdownFormat;
    let output = renderer.render(&ctx, show_links, show_keywords);
    print!("{}", output);
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    url: String,
    urls: Vec<String>,
    output: webfind::cli::OutputArg,
    extract_links: bool,
    extract_keywords: bool,
    dynamic: bool,
    dynamic_wait_ms: u64,
    proxies: Option<String>,
) -> anyhow::Result<()> {
    use webfind::engine::pipeline::FetchPipeline;

    let mut all_urls = vec![url];
    let mut urls = urls;
    all_urls.append(&mut urls);
    all_urls.retain(|u| !u.is_empty());

    let proxy_pool = if let Some(ref list) = proxies {
        Some(ProxyPool::from_list(&split_comma(list))?)
    } else {
        None
    };
    let session_manager = if proxies.is_some() {
        Some(SessionManager::new(StickySessions::Sticky))
    } else {
        None
    };
    let fetcher = if proxies.is_some() {
        Fetcher::new_human(proxy_pool, session_manager, RotateUserAgent::Rotate, 1)?
    } else if dynamic {
        Fetcher::new()?.with_dynamic_fallback(dynamic_wait_ms)
    } else {
        Fetcher::new()?
    };

    if all_urls.len() == 1 {
        let content = fetcher.fetch_url(&all_urls[0]).await?;
        let output_enum: OutputFormat = output.into();
        match output_enum {
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(&content)?;
                print!("{}", json);
            }
            OutputFormat::Report => {
                print_fetch_report(&content, extract_links, extract_keywords);
            }
            OutputFormat::Markdown => {
                print_fetch_markdown(&content, extract_links, extract_keywords);
            }
        }
    } else {
        let pipeline = FetchPipeline::new(fetcher);
        let results = pipeline.fetch_all(&all_urls).await;
        let valid = FetchPipeline::filter_valid(results.clone());
        let (success, failure, errors) = FetchPipeline::summarize(&results);

        println!(
            "Fetched {} URLs: {} success, {} failure",
            all_urls.len(),
            success,
            failure
        );
        for (url, err) in errors {
            println!("  FAIL {}: {}", url, err);
        }
        for content in &valid {
            println!("\n--- {} ---", content.url);
            println!("Title: {}", content.title);
            println!(
                "Words: {} | Grade: {:.1}",
                content.word_count, content.grade_level
            );
        }
    }

    Ok(())
}
