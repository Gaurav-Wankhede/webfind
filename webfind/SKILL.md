# WebFind CLI Skills Reference

**Purpose:** Progressive disclosure for AI agents — load only what you need.
**Token budget:** ~800 tokens (vs 150K+ for full --help).
**Usage:** `WEBFIND_PRINT_SKILLS=1 webfind` or `webfind --print-skills`

---

## 1. Search the Index (BM25 + optional hybrid)

```bash
webfind search "rust async patterns" --hybrid --limit 10
```

**Output schema (JSON):**
```json
{
  "request_id": "uuid",
  "query": "rust async patterns",
  "depth": "Standard",
  "total_results": 42,
  "returned": 10,
  "latency_ms": 127,
  "results": [
    {
      "rank": 1,
      "url": "https://example.com/rust-async",
      "title": "Rust Async Patterns in 2026",
      "snippet": "Comprehensive guide to async/await...",
      "domain": "example.com",
      "score": 0.89,
      "scores": { "bm25": 0.89, "vector": 0.72, "graph": 0.65, "final_score": 0.81 },
      "content": null,
      "keywords": null,
      "metrics": null
    }
  ],
  "suggestions": [],
  "related": [],
  "graph": null,
  "metadata": { "signals_used": ["bm25", "vector", "graph"] }
}
```

**Key flags:** `--hybrid` (BM25+vector+graph), `--limit N`, `--output json|report|markdown`, `--domains "example.com,other.com"`, `--language "en"`

---

## 2. Fetch & Extract a URL (with proxy fallback)

```bash
webfind fetch "https://example.com/article" --dynamic --extract-links --extract-keywords
```

**Output schema (JSON):**
```json
{
  "url": "https://example.com/article",
  "final_url": "https://example.com/article",
  "title": "Article Title",
  "status_code": 200,
  "excerpt": "First 300 chars of extracted content...",
  "published_at": "2026-01-15T10:30:00Z",
  "author": "Jane Doe",
  "site_name": "Example Site",
  "content_text": "Full extracted text...",
  "content_markdown": "# Article Title\n\nFull markdown...",
  "word_count": 2450,
  "reading_time_seconds": 612,
  "language": "en",
  "language_confidence": 0.98,
  "is_valid_content": true,
  "internal_links": ["https://example.com/related"],
  "external_links": ["https://external.com/ref"],
  "keywords": [{"text": "rust", "tfidf_score": 0.42}, {"text": "async", "tfidf_score": 0.38}]
}
```

**Key flags:** `--dynamic` (Chromium fallback), `--dynamic-wait-ms 3000`, `--proxies "http://p1:8080,socks5://p2:1080"`, `--extract-links`, `--extract-keywords`, `--output json|report|markdown`

---

## 3. Research: Crawl + Persist + Search Fresh Content

```bash
webfind research "rust performance" --seed "https://example.com/blog" --max-pages 50 --limit 10 --output /tmp/result.json
```

**Output schema (JSON):** Same as `search` but results are from **freshly
crawled** pages (not pre-indexed). Always includes full content. Every crawled
record is **persisted to the Turso graph store** for durable graph-memory
awareness.

**Key flags:** `--seed URL` (optional — auto-discovers from curated catalog if omitted), `--query`, `--max-pages N`, `--delay 1000`, `--hybrid`, `--limit N`, `--include-graph`, `--include-content` (default true), `--follow-external`, `--topics "rust,performance"`, `--seeds "url1,url2"`, `--dynamic` (CDP Chromium), `--deep` (no timeout/backoff + stealth/scroll), `--graph-store turso|memory`, `--turso-path /path/db`, `--output /path/result.json`

---

## 4. Crawl & Index (build your knowledge base)

```bash
webfind crawl --seed "https://example.com" --depth 3 --max-pages 1000 --hybrid --bulk
```

**Output schema (stdout, human-readable):**
```
Crawl started: https://example.com
Depth: 3 | Max pages: 1000 | Delay: 1000ms
[1/1000] https://example.com (200) 2.4s
[2/1000] https://example.com/page1 (200) 1.1s
...
Crawl complete: 847 pages indexed, 123 failed, 30 skipped
Index: /path/to/webfind.db
```

**Key flags:** `--seed URL`, `--depth N`, `--max-pages N`, `--delay ms`, `--hybrid` (dense vectors), `--bulk` (session-consistent multi-domain), `--graph-store turso|memory`, `--turso-path /path/db`, `--proxies "..."`, `--rotate-ua`, `--respect-robots`, `--follow-external`, `--topics "..."`

---

## Quick Reference: Output Formats

| Format | Use case |
|--------|----------|
| `json` | Machine parsing, agent consumption |
| `report` | Human-readable boxed tables (default) |
| `markdown` | Documentation, LLM context |

---

## Environment Variables (all surfaces)

| Variable | Default | Description |
|----------|---------|-------------|
| `WEBFIND_TURSO_PATH` | `./webfind.db` | Embedded database path |
| `WEBFIND_GRAPH_STORE` | `turso` | `turso` or `memory` |
| `WEBFIND_RATE_LIMIT` | `60` | Requests/second (HTTP API) |
| `WEBFIND_CORS_ORIGINS` | `*` | Comma-separated allowed origins |
| `WEBFIND_DATA_DIR` | `cwd` | Data directory |
| `WEBFIND_CONFIG` | `./webfind.toml` | Config file path |

---

## CLI Commands (pure CLI, no MCP)

| Command | Description |
|---------|-------------|
| `webfind research "q" --output /tmp/r.json` | Crawl + persist + search fresh (single call) |
| `webfind search "q"` | Search pre-indexed graph |
| `webfind fetch URL` | Extract single URL |
| `webfind crawl --seed URL` | Bulk crawl + persist |
| `webfind graph URL` | Traverse persisted link graph |

Agents call these via a harness / system prompt. Use `--output PATH` to write the
JSON result to a file and read it with a file-read tool — no Python/shell parsing.
