use crate::schema::content::{CodeBlockEntity, ImageInfo};
use scraper::Html;

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::engine::fetcher::container::*;
    use crate::engine::fetcher::dom_entities::*;
    use crate::engine::fetcher::entities::*;
    use crate::engine::fetcher::meta::*;
    use crate::engine::fetcher::paywall::*;
    use crate::engine::fetcher::quality_entities::*;
    use crate::engine::fetcher::text_process::*;
    use crate::engine::fetcher::url_util::*;
    use crate::engine::fetcher::Fetcher;

    #[test]
    fn test_extract_meta() {
        let html = "<html><head>\
            <title>My Page Title</title>\
            <meta name=\"description\" content=\"A great page about Rust\">\
            <meta name=\"author\" content=\"Gaurav\">\
            <meta property=\"og:title\" content=\"OG Title\">\
            <link rel=\"canonical\" href=\"https://example.com/canonical\">\
            </head><body><p>Hello world</p></body></html>";
        let doc = Html::parse_document(html);
        let meta = extract_meta(&doc);
        assert_eq!(meta.title.as_deref(), Some("My Page Title"));
        assert_eq!(meta.description.as_deref(), Some("A great page about Rust"));
        assert_eq!(meta.author.as_deref(), Some("Gaurav"));
        assert_eq!(
            meta.canonical_url.as_deref(),
            Some("https://example.com/canonical")
        );
    }

    #[test]
    fn test_extract_open_graph() {
        let html = "<html><head>\
            <meta property=\"og:title\" content=\"OG Title\">\
            <meta property=\"og:description\" content=\"OG Desc\">\
            <meta property=\"og:image\" content=\"https://example.com/img.jpg\">\
            <meta property=\"og:type\" content=\"article\">\
            </head><body></body></html>";
        let doc = Html::parse_document(html);
        let og = extract_open_graph(&doc).unwrap();
        assert_eq!(og.title.as_deref(), Some("OG Title"));
        assert_eq!(og.description.as_deref(), Some("OG Desc"));
        assert_eq!(og.r#type.as_deref(), Some("article"));
    }

    #[test]
    fn test_extract_links() {
        let html = "<html><body>\
            <a href=\"/page1\">Internal</a>\
            <a href=\"https://external.com/page\">External</a>\
            <a href=\"#anchor\">Anchor</a>\
            <a href=\"javascript:void(0)\">JS</a>\
            </body></html>";
        let doc = Html::parse_document(html);
        let (internal, external) = extract_links(&doc, "https://example.com/home");
        assert!(internal.iter().any(|u| u.contains("/page1")));
        assert!(external.iter().any(|u| u.contains("external.com")));
        assert!(!internal.iter().any(|u| u.contains("#anchor")));
    }

    #[test]
    fn test_domain_extraction() {
        assert_eq!(
            extract_domain("https://www.example.com/path"),
            "example.com"
        );
        assert_eq!(
            extract_domain("http://blog.example.co.uk/post"),
            "blog.example.co.uk"
        );
    }

    #[test]
    fn test_word_count() {
        assert_eq!(count_words("Hello world"), 2);
        assert_eq!(count_words("one two three four five"), 5);
    }

    #[test]
    fn test_short_page_is_valid_content() {
        // A short-but-real page (e.g. example.com, ~19 words) must be treated
        // as valid content so webfind_fetch does not mark it invalid and drop
        // it. Regression for the >50 word threshold that pushed the LLM to
        // DuckDuckGo.
        let html = r#"<!DOCTYPE html><html><head><title>Example Domain</title></head>
<body><h1>Example Domain</h1>
<p>This domain is for use in documentation examples without needing permission. Avoid use in operations. Learn more.</p>
</body></html>"#;
        let fetcher = Fetcher::new().unwrap();
        let content = fetcher
            .extract_from_html(
                html,
                "https://example.com/",
                "https://example.com/",
                200,
                true,
                5,
                None,
            )
            .unwrap();
        assert!(content.word_count > 0);
        assert!(
            content.is_valid_content,
            "short page marked invalid (word_count={})",
            content.word_count
        );
    }

    #[test]
    fn test_empty_page_is_invalid_content() {
        let html = "<!DOCTYPE html><html><head><title>Empty</title></head><body></body></html>";
        let fetcher = Fetcher::new().unwrap();
        let content = fetcher
            .extract_from_html(html, "https://x.com/", "https://x.com/", 200, true, 5, None)
            .unwrap();
        assert!(!content.is_valid_content, "empty page must stay invalid");
    }

    #[test]
    fn test_reading_time() {
        assert_eq!(estimate_reading_time(238), 60);
        assert_eq!(estimate_reading_time(100), 26);
    }

    #[test]
    fn test_paywall_detection() {
        assert!(detect_paywall(
            "<html><body><p>This article is for subscribers only</p></body></html>"
        ));
        assert!(!detect_paywall("<html><body><p>Free article</p></body></html>"));
    }

    #[test]
    fn test_paywall_fallback_patterns() {
        assert!(detect_paywall("Please subscribe to continue reading"));
        assert!(detect_paywall("This is premium content behind a paywall"));
        assert!(!detect_paywall("Normal article content"));
    }

    #[test]
    fn test_extract_entities() {
        let text = concat!(
            "Contact support@example.com or sales@corp.io. ",
            "Call +1 555-123-4567. ",
            "Visit https://example.com/pricing or www.example.org. ",
            "The product costs $1,299.00 (or 999 USD). ",
            "Founded on 2024-03-15. Server at 192.168.0.1. ",
            "Follow us @OpenCodeHQ. Office at 123 Main Street, Austin, TX 78701.",
        );
        let e = extract_entities(text);
        assert!(e.emails.iter().any(|m| m == "support@example.com"));
        assert!(e.emails.iter().any(|m| m == "sales@corp.io"));
        assert!(e.phones.iter().any(|m| m.contains("555-123-4567")));
        assert!(e.urls.iter().any(|m| m == "https://example.com/pricing"));
        assert!(e.prices.iter().any(|m| m.contains("1,299")));
        assert!(e.prices.iter().any(|m| m.contains("999")));
        assert!(e.dates.iter().any(|m| m == "2024-03-15"));
        assert!(e.ip_addresses.iter().any(|m| m == "192.168.0.1"));
        assert!(e.social_handles.iter().any(|m| m == "@OpenCodeHQ"));
        assert!(e.addresses.iter().any(|m| m.contains("123 Main Street")));
        assert!(!e.is_empty());
    }

    #[test]
    fn test_extract_entities_avoids_false_positives() {
        // A date must not be classified as a phone; an email local-part must
        // not be classified as a social handle.
        let e = extract_entities("Released 2017-05-13. Reach sales@corp.io directly.");
        assert!(
            e.phones.is_empty(),
            "date misclassified as phone: {:?}",
            e.phones
        );
        assert!(
            !e.social_handles.iter().any(|m| m.contains("corp")),
            "email local-part misclassified as handle: {:?}",
            e.social_handles
        );
        assert!(e.dates.iter().any(|m| m == "2017-05-13"));
        assert!(e.emails.iter().any(|m| m == "sales@corp.io"));
    }

    #[test]
    fn test_extract_entities_empty() {
        let e = extract_entities("Just some plain text with no structured data here.");
        assert!(e.is_empty());
        assert!(e.emails.is_empty() && e.phones.is_empty());
    }

    #[test]
    fn test_extract_technical_entities() {
        let text = "WebFind v0.3.4 is licensed under MIT. Visit https://github.com/Gaurav-Wankhede/webfind for details.";
        let e = extract_entities(text);
        assert!(e.package_versions.iter().any(|v| v == "v0.3.4"));
        assert!(e.licenses.iter().any(|l| l == "MIT"));
        assert!(e.repository_urls.iter().any(|u| u.contains("github.com/Gaurav-Wankhede/webfind")));
        assert!(!e.is_empty());
    }

    #[test]
    fn test_extract_dom_entities() {
        let html = r#"<!DOCTYPE html><html><body>
            <h1 id="intro">Introduction</h1>
            <pre><code class="language-rust">fn main() { println!("hello world"); }</code></pre>
            <table>
                <caption>Features</caption>
                <thead><tr><th>Feature</th><th>Latency</th></tr></thead>
                <tbody><tr><td>Search</td><td>12ms</td></tr></tbody>
            </table>
        </body></html>"#;
        let doc = Html::parse_document(html);
        let (code_blocks, tables, headings) = extract_dom_entities(&doc);
        assert_eq!(headings.len(), 1);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Introduction");
        assert_eq!(headings[0].anchor.as_deref(), Some("intro"));

        assert_eq!(code_blocks.len(), 1);
        assert_eq!(code_blocks[0].language.as_deref(), Some("rust"));
        assert!(code_blocks[0].code.contains("fn main()"));

        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].caption.as_deref(), Some("Features"));
        assert_eq!(tables[0].headers, vec!["Feature", "Latency"]);
        assert_eq!(tables[0].rows, vec![vec!["Search", "12ms"]]);
    }

    #[test]
    fn test_doc_container_and_boilerplate_purging() {
        let html = r#"<!DOCTYPE html><html><head><title>API Docs</title></head>
<body>
<nav><a href="/home">Home</a></nav>
<article>
  <h1>API Overview</h1>
  <p>The API provides programmatic access to your resources.</p>
  <div class="feedback">Was this helpful? Submit feedback to our help community.</div>
</article>
<footer>Footer links and copyright</footer>
</body></html>"#;
        let doc = Html::parse_document(html);
        let extracted = extract_doc_container(&doc);
        assert!(extracted.is_some());
        let (_, text) = extracted.unwrap();
        assert!(text.contains("API Overview"));
        assert!(text.contains("programmatic access to your resources"));
        assert!(!text.to_lowercase().contains("was this helpful"));
        assert!(!text.to_lowercase().contains("submit feedback"));
    }

    #[test]
    fn test_programming_doc_and_research_paper_noise_removal() {
        // Simulates Rustdoc anchor §, copy button, cookie banner, and arXiv abstract
        let html = r##"<!DOCTYPE html><html><head><title>Vec in std::vec</title></head>
<body>
<div id="onetrust-consent-sdk" class="cookie-banner"><p>Accept all tracking cookies</p></div>
<nav class="sidebar"><ul><li>Methods</li></ul></nav>
<main>
  <h1>Struct Vec<a class="anchor" href="#vec">§</a></h1>
  <div class="code-block">
    <button class="copy-button">Copy code</button>
    <pre><code>pub struct Vec&lt;T&gt; { /* fields */ }</code></pre>
  </div>
  <blockquote class="abstract">
    <span class="descriptor">Abstract:</span> We present a new deep learning architecture.
  </blockquote>
  <div class="extra-services">
    <a href="/pdf/123">Download PDF</a>
    <button>Export BibTeX</button>
  </div>
</main>
</body></html>"##;
        let doc = Html::parse_document(html);
        let extracted = extract_doc_container(&doc);
        assert!(extracted.is_some());
        let (_, text) = extracted.unwrap();

        // Must keep core content
        assert!(text.contains("Struct Vec"));
        assert!(text.contains("pub struct Vec"));
        assert!(text.contains("We present a new deep learning architecture"));

        // Must purge all noise elements
        assert!(!text.contains("§"), "Anchor symbol § must be stripped");
        assert!(!text.contains("Copy code"), "Copy button must be stripped");
        assert!(!text.contains("Accept all tracking cookies"), "Cookie banner must be stripped");
        assert!(!text.contains("Download PDF"), "arXiv extra services must be stripped");
        assert!(!text.contains("Export BibTeX"), "BibTeX export buttons must be stripped");
    }

    #[test]
    fn test_wikipedia_and_stackoverflow_noise_removal() {
        let html = r##"<!DOCTYPE html><html><head><title>Transformer Architecture</title></head>
<body>
<div id="mw-navigation">Navigation sidebar and portals</div>
<div class="mw-parser-output">
  <h2>Overview<span class="mw-editsection"><a href="#">[edit]</a></span></h2>
  <p>Transformers use multi-head self-attention.<sup class="reference"><a href="#cite">[1]</a></sup></p>
  <div class="post-signature user-info">answered 2 hours ago by user123 (15.2k reputation)</div>
  <div class="js-voting-container"><button class="js-vote-count">142 votes</button></div>
</div>
</body></html>"##;
        let doc = Html::parse_document(html);
        let extracted = extract_doc_container(&doc);
        assert!(extracted.is_some());
        let (_, text) = extracted.unwrap();

        // Must retain authentic core knowledge
        assert!(text.contains("Overview"));
        assert!(text.contains("Transformers use multi-head self-attention"));

        // Must strip Wikipedia and SO UI clutter
        assert!(!text.contains("[edit]"), "Wikipedia [edit] links must be stripped");
        assert!(!text.contains("[1]"), "Wikipedia reference superscripts must be stripped");
        assert!(!text.contains("user123"), "User reputation flair must be stripped");
        assert!(!text.contains("142 votes"), "Vote counters must be stripped");
        assert!(!text.contains("Navigation sidebar"), "Wikipedia navigation must be stripped");
    }

    #[test]
    fn test_blog_and_forum_noise_removal() {
        let html = r##"<!DOCTYPE html><html><head><title>System Architecture Guide</title></head>
<body>
<article class="post">
  <h1>Distributed Raft Consensus</h1>
  <div class="social-share-bar"><button>Share on Twitter</button></div>
  <p>Raft guarantees strong consistency across replicas using leader heartbeats.</p>
  <div class="reactions-widget"><button class="clap-button">500 claps</button></div>
  <div class="author-card">Written by Alex. Passionate about cloud infra.</div>
  <div class="newsletter-subscribe">Ready for more? Subscribe for free!</div>
</article>
</body></html>"##;
        let doc = Html::parse_document(html);
        let extracted = extract_doc_container(&doc);
        assert!(extracted.is_some());
        let (_, text) = extracted.unwrap();

        // Must retain core knowledge
        assert!(text.contains("Distributed Raft Consensus"));
        assert!(text.contains("Raft guarantees strong consistency across replicas"));

        // Must strip social, claps, author promo, and newsletter prompts
        assert!(!text.contains("Share on Twitter"));
        assert!(!text.contains("500 claps"));
        assert!(!text.contains("Passionate about cloud infra"));
        assert!(!text.contains("Subscribe for free"));
    }

    #[test]
    fn test_cross_domain_noise_removal() {
        let html = r##"<!DOCTYPE html><html><head><title>Property Deed and Mortgage Overview</title></head>
<body>
<div class="affiliate-disclosure"><p>Advertiser disclosure: We may receive compensation from partners.</p></div>
<article>
  <h1>Understanding Title Deeds</h1>
  <div class="lead-modal popup"><p>Get pre-approved for a mortgage today!</p></div>
  <p>A deed is a signed legal document that transfers ownership of an asset to a new owner.</p>
  <div class="mortgage-calculator widget">Calculate your monthly rate</div>
  <div class="disclaimer-footer">This information is not intended to be legal advice.</div>
</article>
</body></html>"##;
        let doc = Html::parse_document(html);
        let extracted = extract_doc_container(&doc);
        assert!(extracted.is_some());
        let (_, text) = extracted.unwrap();

        // Must retain authentic core knowledge
        assert!(text.contains("Understanding Title Deeds"));
        assert!(text.contains("A deed is a signed legal document that transfers ownership"));

        // Must strip affiliate, lead modal, mortgage widget, and legal advice disclaimers
        assert!(!text.contains("Advertiser disclosure"));
        assert!(!text.contains("Get pre-approved for a mortgage"));
        assert!(!text.contains("Calculate your monthly rate"));
        assert!(!text.contains("legal advice"));
    }

    #[test]
    fn test_quality_entities_from_doc() {
        let html = r##"<!DOCTYPE html><html><head><title>Guide</title></head>
<body><article>
<h2>Quickstart Example</h2>
<pre><code class="language-rust">fn main() { println!("hi"); }</code></pre>
<h2>What is Raft?</h2>
<p>Raft is a consensus algorithm.</p>
<h2>How to install</h2>
<ol><li>Download the binary</li><li>Run it</li></ol>
<blockquote>Warning: do not run as root.</blockquote>
<pre><code class="language-mermaid">graph TD; A-->B;</code></pre>
</article></body></html>"##;
        let doc = Html::parse_document(html);
        let (examples, faqs, steps, callouts) = extract_quality_entities(&doc);
        assert_eq!(examples.len(), 2);
        assert_eq!(
            examples[0].context_heading.as_deref(),
            Some("Quickstart Example")
        );
        assert_eq!(examples[0].language.as_deref(), Some("rust"));
        assert_eq!(faqs.len(), 1);
        assert_eq!(faqs[0].question, "What is Raft?");
        assert_eq!(faqs[0].answer, "Raft is a consensus algorithm.");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].position, 1);
        assert_eq!(steps[0].title, "Download the binary");
        assert_eq!(callouts.len(), 1);
        assert_eq!(callouts[0].kind, "warning");
    }

    #[test]
    fn test_classify_diagrams() {
        let blocks = vec![
            CodeBlockEntity {
                language: Some("mermaid".into()),
                code: "graph TD; A-->B;".into(),
                line_count: 1,
            },
            CodeBlockEntity {
                language: Some("rust".into()),
                code: "fn main() {}".into(),
                line_count: 1,
            },
        ];
        let images = vec![ImageInfo {
            url: "https://x.com/architecture-diagram.png".into(),
            alt: Some("System architecture".into()),
            width: None,
            height: None,
        }];
        let diagrams = classify_diagrams(&blocks, &images);
        assert_eq!(diagrams.len(), 2);
        assert_eq!(diagrams[0].kind, "mermaid");
        assert_eq!(diagrams[1].kind, "image");
    }

    #[test]
    fn test_math_entities_skip_prices() {
        let entities = extract_entities("Einstein wrote $$E=mc^2$$ in 1905. It costs $19.99 today.");
        assert_eq!(entities.maths.len(), 1);
        assert!(entities.maths[0].display);
        assert!(entities.maths[0].tex.contains("E=mc^2"));
    }

    #[test]
    fn test_quality_entities_avoid_false_positives() {
        // photograph (contains "graph"), rhetorical fragment, nav list,
        // pull-quote, and prose brackets must not inflate entity counts.
        let html = r##"<!DOCTYPE html><html><head><title>FP</title></head>
<body><article>
<img src="team-photograph.jpg" alt="Team photograph retreat" />
<h2>What now?</h2>
<p>Ok.</p>
<h2>Latest News</h2>
<ol><li>Launch</li><li>Update</li></ol>
<blockquote>Great product, five stars.</blockquote>
<pre><code>plain snippet</code></pre>
</article></body></html>"##;
        let doc = Html::parse_document(html);
        let diagrams = classify_diagrams(
            &extract_dom_entities(&doc).0,
            &extract_images(&doc),
        );
        assert!(diagrams.is_empty(), "photo misclassified: {diagrams:?}");
        let (examples, faqs, steps, callouts) = extract_quality_entities(&doc);
        assert!(faqs.is_empty(), "rhetorical fragment misclassified: {faqs:?}");
        assert!(steps.is_empty(), "nav list misclassified: {steps:?}");
        assert_eq!(callouts.len(), 1);
        assert_eq!(callouts[0].kind, "quote");
        assert_eq!(examples.len(), 1, "code without heading context lost");
        let maths = extract_entities("see (figure 3) for details");
        assert!(maths.maths.is_empty());
    }

    #[test]
    fn test_quality_entities_golden_counts() {
        // WCXB-style miniature: exact TP counts per entity on one fixture.
        let html = r##"<!DOCTYPE html><html><head><title>Golden</title></head>
<body><article>
<h2>Install Example</h2>
<pre><code class="language-bash">cargo install webfind</code></pre>
<h2>How do I configure it?</h2>
<p>Pass flags on the command line.</p>
<h2>How to deploy</h2>
<ol><li>Build the binary</li><li>Ship it</li></ol>
<blockquote>[!WARNING] Back up first.</blockquote>
<pre><code class="language-mermaid">graph TD; A-->B;</code></pre>
</article></body></html>"##;
        let doc = Html::parse_document(html);
        let (code_blocks, _, _) = extract_dom_entities(&doc);
        let diagrams = classify_diagrams(&code_blocks, &[]);
        let (examples, faqs, steps, callouts) = extract_quality_entities(&doc);
        assert_eq!(examples.len(), 2);
        assert_eq!(diagrams.len(), 1);
        assert_eq!(diagrams[0].kind, "mermaid");
        assert_eq!(faqs.len(), 1);
        assert_eq!(steps.len(), 2);
        assert_eq!(callouts.len(), 1);
        assert_eq!(callouts[0].kind, "warning");
    }
}

