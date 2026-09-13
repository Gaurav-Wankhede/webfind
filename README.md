# WebFind

> Self-hosted web research for free & local LLMs — single binary, zero cost, no cloud, no auth.

WebFind gives **free and locally-hosted models** the live web research that paid plans gate behind web search. When you use a paid model (Claude, Codex), web search is built into the harness. But on **free / local models**, web search doesn't work — that's exactly the gap WebFind fills.

WebFind crawls, indexes, ranks, and searches the live web, then hands fresh cited results to your model over the **pure CLI**. Every crawled record is **persisted to the embedded Turso graph store**, so each run builds durable graph-memory awareness that later searches reuse. Everything runs on your machine: no API keys, no cloud, no authentication.

```
Local Model (harness + system prompt) → webfind research (single call) → Crawl & Persist to Turso Graph → BM25 + Vector + PageRank → JSON result file → back to the model
```

---

## 🌟 Key Features

- 🖥️ **Google-Style Web Interface**: Modern, responsive Web UI built with Axum, HTMX, Tailwind CSS, real-time SSE research streaming, auto-complete search suggestions, and category filtering.
- 🧪 **Pure CLI (no MCP)**: One command crawls, persists to the DB, indexes, ranks, and emits results. No server to keep alive, no round-trip state. Local models call the binary directly via a harness / system prompt.
- 🧠 **Durable Graph Memory**: Every record is persisted to the Turso DB (`url_nodes` + `link_edges` + `page_content`). No temp JSON — records live in the database and future queries search the accumulated graph.
- ⚡ **Hybrid BM25 + Dense Vector Search**: High-performance full-text search combined with dense vector embeddings (`fastembed-rs`) for semantic search re-ranking, fused with Reciprocal Rank Fusion.
- 🕸️ **Deep Web Crawler**: High-concurrency crawler with `robots.txt` compliance, rate limiting, domain session stickiness, User-Agent rotation, proxy CIDR pool routing, and headless Chromium (CDP) for JavaScript-rendered / infinite-scroll pages.
- ⏱️ **No-Timeout Deep Research**: `--deep` removes the crawl deadline and backoff caps (90s request timeout) so long-running investigations can finish.
- 📊 **Turso Embedded Graph & PageRank**: Single-file SQLite (Turso/libSQL) for the link graph, vector index (DiskANN), full-text index (FTS5), and PageRank — zero external database.
- 🎯 **Clean Noise-Free Extraction for AI Agents**: Strips scripts, styles, ads, navigation, headers, and footers. Focuses on core structural elements to deliver high-density, token-efficient content (inspired by Firecrawl).
- 🔒 **Encryption at Rest**: Optional SQLCipher AES-256-CBC encryption for the Turso database file.

---

## 🚀 Quick Start & Global Installation

Install `webfind` globally across **Linux, macOS, and Windows** to your system's global binary folder, automatically wiping target build artifacts:

```bash
git clone https://github.com/Gaurav-Wankhede/WebFind.git
cd WebFind
cargo install --path . --force && cargo clean
```

> **Universal Global Binary Locations (Already in your `$PATH`):**
> - **macOS / Linux**: `~/.cargo/bin/webfind`
> - **Windows**: `%USERPROFILE%\.cargo\bin\webfind.exe`
>
> Once installed, run `webfind` from any directory or terminal session without navigating to the project folder.

### Run a research query (single call — crawls, persists to graph, ranks, writes JSON):

```bash
webfind research "Rust async runtime tokio 2026" \
  --max-pages 20 --delay 300 --deep --dynamic \
  --output /tmp/webfind_result.json
```

Read `/tmp/webfind_result.json` — records are already persisted to `webfind.db` for graph-memory awareness.

## 🌐 Optional: HTTP + Web UI (Docker)

For the Google-style Web UI and the REST API, run the self-contained service with Docker Compose:

```bash
docker compose up -d --build
```

### Services & Endpoints

| Service | Access URL | Description |
|---|---|---|
| 🌐 **Web UI** | `http://localhost:5750` | Google-style search engine interface with live SSE research streaming |
| ⚡ **REST API** | `http://localhost:5748` | Direct REST endpoints (`/search`, `/research`, `/health`) |

All data (Turso DB, cache) lives in the `/data` volume. Backup is a file copy: `cp webfind.db backup.db`.

---

## 🖥️ Web Interface (GUI)

Access the Web UI at **`http://localhost:5750`**:

- **Instant Search & Autocomplete**: Real-time suggestion dropdown as you type.
- **Category Filtering**: Filter results by Tech, News, Science, Business, and Custom categories.
- **Live SSE Research Streaming**: Watch WebFind crawl seed URLs, discover links, and extract content live with progress tracking.
- **Rendered Content & Snippets**: Read extracted content directly with readability scores and highlighted query terms.

---

## 🧪 AI Agent Integration (Pure CLI)

WebFind is a **pure CLI** system — there is no MCP server. Local models call the `webfind` binary directly via a harness / system prompt.

### The single-call pattern

Run one CLI call, write the JSON result to a file, then read that file with a file-read tool:

```bash
webfind research "your query here" \
  --max-pages 20 --delay 300 --deep --dynamic \
  --output /tmp/webfind_result.json
```

- **stdout / `--output` file** = one JSON document (`query`, `total_results`, `results[]`, `searched_at`).
- **stderr** = progress (`Researching: …`, `Crawl + persist complete …`), logs, warnings.
- Records are persisted to the Turso graph store; future `webfind search` queries reuse the accumulated graph.

**Key flags:** `--max-pages`, `--limit`, `--include-content` (default true), `--dynamic` (CDP Chromium), `--deep` (no timeout + stealth/scroll), `--seed` (auto-discovers from curated catalog if omitted), `--graph-store turso|memory`, `--turso-path`, `--output`.

---

## 💻 CLI Command Guide

```bash
# Deep research crawl: crawl + persist to graph + search, write JSON result
webfind research "rust async runtime" --max-pages 20 --deep --dynamic --output /tmp/r.json

# Search the accumulated index/graph
webfind search "rust async" --limit 10 --hybrid

# Crawl & persist a domain graph into the embedded Turso store
webfind crawl --seed https://news.ycombinator.com --depth 3 --max-pages 100

# Fetch page content with headless Chromium for JS-heavy SPAs
webfind fetch https://react.dev --dynamic --dynamic-wait-ms 3000

# Traverse the link graph in Turso
webfind graph https://doc.rust-lang.org/ --depth 2

# Check engine and index status
webfind status
```

---

## 🌐 REST API Endpoints (HTTP mode)

| Endpoint | Method | Query Parameters / Body | Description |
|---|---|---|---|
| `/health` | GET | - | Health status & index document count |
| `/search` | GET | `q`, `limit`, `hybrid` | Search local index |
| `/research` | GET | `seed`, `q`, `max_pages`, `depth` | Live crawl, index, and return ranked search |
| `/api/web/suggest` | GET | `q` | Real-time autocomplete suggestions |
| `/api/web/categories` | GET | - | Retrieve domain categories & filter stats |
| `/api/web/research/stream` | GET | `seed`, `q`, `depth`, `max_pages` | Server-Sent Events (SSE) stream for live research |

---

## 🏗️ Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│                          Clients & Consumers                           │
│  ┌───────────────────────────────┐   ┌──────────────────────────────┐  │
│  │  AI Agent (pure CLI, harness) │   │  Human Web UI (HTMX + SSE)   │  │
│  └───────────────┬───────────────┘   └──────────────┬───────────────┘  │
└──────────────────┼──────────────────────────────────┼──────────────────┘
                   │  webfind research --output JSON  │ GET / HTTP
┌──────────────────▼──────────────────────────────────▼──────────────────┐
│                        WebFind Engine (Rust)                           │
│                                                                        │
│  ┌──────────────────────┐  ┌────────────────────┐  ┌────────────────┐  │
│  │   Crawler Engine     │  │  Headless Browser  │  │   Proxy Pool   │  │
│  │ (robots.txt, delay)  │  │ (Chromium / CDP)   │  │ (CIDR / SOCKS) │  │
│  └──────────┬───────────┘  └─────────┬──────────┘  └────────────────┘  │
│             ▼                        ▼                                 │
│  ┌──────────────────────────────────────────────┐                      │
│  │          Indexer (BM25 + Vector)             │                      │
│  │     BM25 + FastEmbed / DiskANN               │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │                                              │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │          Hybrid Ranker & Scorer (RRF)        │                      │
│  │      (BM25 + Dense Vector + PageRank)        │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │ embedded Turso (webfind.db)                  │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │              Turso / libSQL Store            │                      │
│  │   url_nodes · link_edges · page_content      │                      │
│  │   Link Graph, FTS5, DiskANN Vector, PageRank │                      │
│  └──────────────────────────────────────────────┘                      │
└────────────────────────────────────────────────────────────────────────┘
```

---

## ⚙️ Environment Variables

| Variable | Default | Description |
|---|---|---|
| `WEBFIND_DATA_DIR` | cwd | Directory for the Turso DB and cache storage |
| `WEBFIND_GRAPH_STORE` | `turso` | Graph backend (`turso` or `memory`) |
| `WEBFIND_TURSO_PATH` | `webfind.db` | Path to the embedded Turso/libSQL database file |
| `WEBFIND_GUI_PORT` | `4749` | Web UI HTTP server port |
| `WEBFIND_RATE_LIMIT` | `60` | Per-IP rate limiting (requests per second; `0` disables) |
| `WEBFIND_BODY_LIMIT` | `1048576` | Max request body size in bytes |
| `WEBFIND_CORS_ORIGINS` | *(empty)* | Comma-separated allowed origins (empty = restrictive, no CORS) |
| `WEBFIND_CONFIG` | `./webfind.toml` | Config file path |

---

## 📜 License

[MIT](LICENSE)
