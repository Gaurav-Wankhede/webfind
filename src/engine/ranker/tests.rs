use super::Ranker;
use super::{CandidateDescriptor, ZeroHeapFunnelEngine};
use std::collections::HashMap;


    #[test]
    fn test_freshness_recent() {
        let recent = chrono::Utc::now() - chrono::Duration::hours(1);
        let score = Ranker::freshness_score(recent);
        assert!(score > 0.9, "recent page should score high: {}", score);
    }

    #[test]
    fn test_freshness_old() {
        let old = chrono::Utc::now() - chrono::Duration::days(400);
        let score = Ranker::freshness_score(old);
        assert!(score < 0.15, "old page should score low: {}", score);
    }

    #[test]
    fn test_freshness_one_year() {
        let one_year = chrono::Utc::now() - chrono::Duration::days(365);
        let score = Ranker::freshness_score(one_year);
        assert!(
            score > 0.1 && score < 0.15,
            "one year old should score ~0.13: {}",
            score
        );
    }

    #[test]
    fn test_freshness_one_month() {
        let one_month = chrono::Utc::now() - chrono::Duration::days(30);
        let score = Ranker::freshness_score(one_month);
        assert!(score > 0.8, "one month old should score high: {}", score);
    }

    #[test]
    fn test_quality_easy_read() {
        let score = Ranker::quality_score(70.0, 8.0);
        assert!(score > 0.6, "easy readable content should score well");
    }

    #[test]
    fn test_quality_hard_read() {
        let score = Ranker::quality_score(20.0, 18.0);
        assert!(score < 0.3, "hard content should score poorly");
    }

    #[test]
    fn test_freshness_domination() {
        let now = chrono::Utc::now();
        let fresh = Ranker::freshness_score(now);
        let stale = Ranker::freshness_score(now - chrono::Duration::days(365));
        assert!(
            fresh > stale * 5.0,
            "fresh should dominate stale: {} vs {}",
            fresh,
            stale
        );
    }

    #[test]
    fn test_rank_applies_graph_boost() {
        use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
        use crate::schema::response::{ContentMetrics, ScoreBreakdown, SearchResult};
        let now = chrono::Utc::now();
        let base = SearchResult {
            rank: 0,
            url: "https://example.com/base".to_string(),
            title: "Base".to_string(),
            snippet: "".to_string(),
            domain: "example.com".to_string(),
            published_at: Some(now),
            modified_at: None,
            crawled_at: now,
            author: None,
            site_name: None,
            score: 0.0,
            scores: ScoreBreakdown {
                bm25: Some(0.5),
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                ax_score: None,
                final_score: 0.0,
            },
            content: None,
            tier: None,
            examples: None,
            keywords: None,
            metrics: Some(ContentMetrics {
                reading_ease: 70.0,
                grade_level: 8.0,
                fog_index: 8.0,
                sentence_count: 10,
                avg_words_per_sentence: 12.0,
                language: "en".to_string(),
                language_confidence: 0.99,
                has_structured_data: false,
                schema_type: None,
            }),
            favicon: None,
            thumbnail: None,
            llms_txt: None,
            ai_catalog: None,
            openapi_spec: None,
            mcp_server: None,
            language: "en".to_string(),
            content_type: "text".to_string(),
        };
        let mut boosted = base.clone();
        boosted.url = "https://example.com/boosted".to_string();
        boosted.scores.bm25 = Some(0.5);
        let request = SearchRequest {
            query: "test".to_string(),
            depth: SearchDepth::Standard,
            limit: 10,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content: false,
            include_graph: false,
            include_keywords: false,
            include_metrics: false,
            hybrid: false,
        };
        let mut graph_scores = HashMap::new();
        let _ = graph_scores.insert(base.url.clone(), 0.0);
        let _ = graph_scores.insert(boosted.url.clone(), 1.0);

        let ranker = Ranker::new();
        let ranked = ranker.rank(
            vec![base.clone(), boosted.clone()],
            &request,
            Some(&graph_scores),
            None,
        );

        assert_eq!(ranked[0].url, boosted.url);
        assert!(ranked[0].scores.graph.unwrap() > ranked[1].scores.graph.unwrap());
    }

    #[test]
    fn test_diversity_penalty_does_not_dominate_relevance() {
        use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
        use crate::schema::response::{ContentMetrics, ScoreBreakdown, SearchResult};
        let now = chrono::Utc::now();
        let dup = SearchResult {
            rank: 0,
            url: "https://example.com/a".to_string(),
            title: "A".to_string(),
            snippet: "".to_string(),
            domain: "example.com".to_string(),
            published_at: Some(now),
            modified_at: None,
            crawled_at: now,
            author: None,
            site_name: None,
            score: 0.0,
            scores: ScoreBreakdown {
                bm25: Some(0.9),
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                ax_score: None,
                final_score: 0.0,
            },
            content: None,
            tier: None,
            examples: None,
            keywords: None,
            metrics: Some(ContentMetrics {
                reading_ease: 70.0,
                grade_level: 8.0,
                fog_index: 8.0,
                sentence_count: 10,
                avg_words_per_sentence: 12.0,
                language: "en".to_string(),
                language_confidence: 0.99,
                has_structured_data: false,
                schema_type: None,
            }),
            favicon: None,
            thumbnail: None,
            llms_txt: None,
            ai_catalog: None,
            openapi_spec: None,
            mcp_server: None,
            language: "en".to_string(),
            content_type: "text".to_string(),
        };
        let mut dup2 = dup.clone();
        dup2.url = "https://example.com/b".to_string();
        dup2.scores.bm25 = Some(0.85);
        let mut unique = dup.clone();
        unique.url = "https://other.com/c".to_string();
        unique.domain = "other.com".to_string();
        unique.scores.bm25 = Some(0.7);

        let request = SearchRequest {
            query: "test".to_string(),
            depth: SearchDepth::Standard,
            limit: 10,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content: false,
            include_graph: false,
            include_keywords: false,
            include_metrics: false,
            hybrid: false,
        };
        let ranker = Ranker::new();
        let ranked = ranker.rank(vec![dup, dup2, unique], &request, None, None);

        // The single unique result should outrank the slightly weaker duplicate
        // because diversity penalty pushes the second example.com page below it.
        assert_eq!(ranked[0].url, "https://example.com/a");
        assert_eq!(ranked[1].url, "https://other.com/c");
        assert_eq!(ranked[2].url, "https://example.com/b");
    }

    #[test]
    fn test_ax_score_machine_readability() {
        use crate::schema::response::{ContentBlock, SearchResult};
        let now = chrono::Utc::now();
        let mut r = SearchResult {
            rank: 1,
            url: "https://api.example.com/docs".to_string(),
            title: "API Docs".to_string(),
            snippet: "Documentation".to_string(),
            domain: "api.example.com".to_string(),
            published_at: Some(now),
            modified_at: None,
            crawled_at: now,
            author: None,
            site_name: None,
            score: 0.0,
            scores: crate::schema::response::ScoreBreakdown {
                bm25: None,
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                ax_score: None,
                final_score: 0.0,
            },
            content: Some(ContentBlock {
                text: "### Events\n```bash\ncurl https://api.example.com/events\n```".to_string(),
                excerpt: "Events API".to_string(),
                word_count: 10,
                reading_time_seconds: 3,
                html: None,
                markdown: Some("### Events\n```bash\ncurl https://api.example.com/events\n```".to_string()),
            }),
            tier: None,
            examples: None,
            keywords: None,
            metrics: None,
            favicon: None,
            thumbnail: None,
            llms_txt: Some("https://api.example.com/llms.txt".to_string()),
            ai_catalog: None,
            openapi_spec: Some("https://api.example.com/openapi.json".to_string()),
            mcp_server: Some("npx -y @example/mcp-server".to_string()),
            language: "en".to_string(),
            content_type: "text".to_string(),
        };

        let ax = Ranker::calculate_ax_score(&r);
        assert!(ax.is_some());
        let val = ax.unwrap();
        // Manifest (0.35) + Markdown/code (0.25) + OpenAPI/MCP (0.20) = 0.80
        assert!((val - 0.80).abs() < 1e-5);

        r.llms_txt = None;
        r.openapi_spec = None;
        r.mcp_server = None;
        r.content = None;
        assert_eq!(Ranker::calculate_ax_score(&r), None);
    }

    #[test]
    fn test_zero_heap_funnel_gate_filtering() {

        let mut funnel = ZeroHeapFunnelEngine::new(0.60, 0.45);

        // Candidate 1: High semantic & high quality (passes)
        let pass_cand = CandidateDescriptor {
            index: 0,
            vector_sim: 0.85,
            quality_metric: 0.75,
            bm25_score: 0.5,
            freshness_score: 1.0,
            graph_score: 0.2,
            example_count: 2,
            final_rank_score: 0.0,
            passed_gates: false,
        };

        // Candidate 2: Low semantic (fails Gate 1)
        let fail_vector = CandidateDescriptor {
            index: 1,
            vector_sim: 0.30,
            quality_metric: 0.80,
            bm25_score: 0.5,
            freshness_score: 1.0,
            graph_score: 0.2,
            example_count: 0,
            final_rank_score: 0.0,
            passed_gates: false,
        };

        // Candidate 3: Low quality (fails Gate 2)
        let fail_quality = CandidateDescriptor {
            index: 2,
            vector_sim: 0.90,
            quality_metric: 0.20,
            bm25_score: 0.5,
            freshness_score: 1.0,
            graph_score: 0.2,
            example_count: 0,
            final_rank_score: 0.0,
            passed_gates: false,
        };

        assert!(funnel.ingest_candidate(pass_cand));
        assert!(!funnel.ingest_candidate(fail_vector));
        assert!(!funnel.ingest_candidate(fail_quality));

        let ranked = funnel.rank_and_slice();
        assert_eq!(ranked.len(), 3);
        // The one that passed both gates must be sorted first
        assert!(ranked[0].passed_gates);
        assert_eq!(ranked[0].index, 0);
        // Failed gate candidates must follow
        assert!(!ranked[1].passed_gates);
        assert!(!ranked[2].passed_gates);
    }

    #[test]
    fn test_extract_examples() {
        let text = "Here is a guide:\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nAnd a CLI command:\n```bash\ncargo build --release\n```\n";
        let examples = Ranker::extract_examples(text);
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[0].language.as_deref(), Some("rust"));
        assert!(examples[0].snippet.contains("println"));
        assert_eq!(examples[1].language.as_deref(), Some("bash"));
        assert!(examples[1].snippet.contains("cargo build"));
    }
