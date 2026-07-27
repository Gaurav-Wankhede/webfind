use std::collections::HashMap;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Tiny HTTP/1.1 test server that exposes:
/// - `/robots.txt`
/// - `/sitemap.xml`
/// - `/` (home) linking to /page1 and /page2
/// - `/page1`, `/page2`
/// - `/private/secret` (blocked by robots.txt)
pub async fn start_test_server() -> (tokio::task::JoinHandle<()>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        let mut pages: HashMap<String, (String, &'static str)> = HashMap::new();
        pages.insert(
            "/robots.txt".to_string(),
            (
                "User-agent: *\nDisallow: /private/\nSitemap: http://127.0.0.1:PLACEHOLDER/sitemap.xml\nCrawl-delay: 0.1\n".to_string(),
                "text/plain; charset=utf-8",
            ),
        );
        pages.insert(
            "/sitemap.xml".to_string(),
            (
                r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>http://127.0.0.1:PLACEHOLDER/page1</loc>
    <lastmod>2024-01-15T00:00:00+00:00</lastmod>
    <changefreq>daily</changefreq>
    <priority>0.9</priority>
  </url>
  <url>
    <loc>http://127.0.0.1:PLACEHOLDER/page2</loc>
    <priority>0.5</priority>
  </url>
</urlset>"#
                    .to_string(),
                "application/xml; charset=utf-8",
            ),
        );
        pages.insert(
            "/".to_string(),
            (
                r#"<!DOCTYPE html>
<html><head><title>Home</title></head><body>
<h1>Welcome</h1>
<p>This is the home page of the bulk crawler integration test server. It contains enough text to satisfy the validity threshold so that the fetcher marks the page as real content. The crawler needs at least fifty words in the body to consider a page worth keeping. We add extra sentences here to make sure the word count easily crosses that minimum. From here we can navigate to other internal pages.</p>
<ul><li><a href="/page1">Page One</a></li><li><a href="/page2">Page Two</a></li><li><a href="/private/secret">Secret</a></li></ul>
</body></html>"#.to_string(),
                "text/html; charset=utf-8",
            ),
        );
        pages.insert(
            "/page1".to_string(),
            (
                r#"<!DOCTYPE html>
<html><head><title>Page One</title></head><body>
<h1>Page One</h1>
<p>This is the first linked page. It also contains plenty of readable text so that the content extractor classifies it as valid. The bulk crawler should discover this page from the home page link. We include additional filler words to push the word count above the required fifty word threshold used by the fetcher.</p>
</body></html>"#.to_string(),
                "text/html; charset=utf-8",
            ),
        );
        pages.insert(
            "/page2".to_string(),
            (
                r#"<!DOCTYPE html>
<html><head><title>Page Two</title></head><body>
<h1>Page Two</h1>
<p>This is the second linked page. Like the others it contains enough words to be considered valid content during the integration test run. We add more sentences here so that readability extraction returns enough text for the fetcher to mark the result as valid content. Additional filler text follows to guarantee the word count exceeds the fifty word minimum threshold.</p>
</body></html>"#.to_string(),
                "text/html; charset=utf-8",
            ),
        );
        pages.insert(
            "/private/secret".to_string(),
            (
                r#"<!DOCTYPE html>
<html><head><title>Secret</title></head><body>
<h1>Secret Page</h1>
<p>This page should not be crawled because robots.txt disallows the private directory. We add enough filler words here so that if it were accidentally crawled it would still be considered valid content, proving the robots filter works. The page keeps going with extra sentences to make absolutely sure the word count exceeds the fifty word threshold used by the fetcher for validity.</p>
</body></html>"#.to_string(),
                "text/html; charset=utf-8",
            ),
        );

        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let pages = pages.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");

                let host = request
                    .lines()
                    .find(|line| line.to_lowercase().starts_with("host:"))
                    .map(|line| line.split_once(':').map(|(_, v)| v.trim()).unwrap_or(""))
                    .unwrap_or("127.0.0.1");

                let (status, content_type, body) = if let Some((body, ct)) = pages.get(path) {
                    (
                        "200 OK",
                        *ct,
                        body.replace("PLACEHOLDER", host.split(':').nth(1).unwrap_or("80")),
                    )
                } else {
                    ("404 Not Found", "text/plain", "Not found".to_string())
                };

                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    content_type,
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    (handle, format!("http://127.0.0.1:{}", port))
}
