# WebFind

> Self-hosted native search engine & deep research assistant for AI agents and humans — Docker, zero cost.

WebFind gives AI agents and human users fresh, cited evidence from the web instead of stale LLM training data. Crawl, index, rank, and search the live web — then consume results via a Google-style Web UI or hand them to your agent via MCP.

```
Agent (MCP) / Human (Web UI) → WebFind Crawl & Discovery → Tantivy BM25 + Vector HNSW → SurrealDB Link Graph & PageRank → Ranked Cited Results
```

---

## 🌟 Key Features

- 🌐 **Google-Style Web Interface**: Modern, responsive Web UI built with Axum, HTMX, Tailwind CSS, real-time SSE research streaming, auto-complete search suggestions, and category filtering.
- 🤖 **MCP Native (Streamable HTTP & Stdio)**: Seamless integration with OpenCode, Claude Desktop, Claude Code, Cursor, and any MCP-compatible agent platform.
- ⚡ **Hybrid BM25 + Dense Vector Search**: High-performance full-text search powered by Tantivy combined with dense vector embeddings (`fastembed-rs`) for semantic search re-ranking.
- 🕸️ **Deep Web Crawler**: High-concurrency crawler with `robots.txt` compliance, rate limiting, domain session stickiness, User-Agent rotation, proxy CIDR pool routing, and headless Chromium fallback for JavaScript-rendered SPA pages.
- 📊 **SurrealDB Graph Topology & PageRank**: Link graph storage in SurrealDB for link traversal, domain mapping, and PageRank score calculation.
- 🎯 **Clean Noise-Free Extraction for AI Agents**: Strips out scripts, styles, ads, navigation bars, and headers/footers. Focuses strictly on core structural elements (`<h1-h6>`, `<p>`, `<div>`, `<span>`) to deliver high-density, token-efficient Markdown content to LLM context windows (inspired by Firecrawl).
- 📦 **Common Crawl Import**: Tools to import and index massive datasets directly from Common Crawl (`CC-MAIN`).

---

## 🚀 Quick Start (Docker)

Spin up WebFind, SurrealDB, and automatic schema initialization with Docker Compose:

```bash
git clone https://github.com/Gaurav-Wankhede/WebFind.git
cd WebFind
docker compose up -d --build
```

### Services & Endpoints

| Service | Access URL | Description |
|---|---|---|
| 🌐 **Web UI** | `http://localhost:5750` | Google-style search engine interface with live SSE research streaming |
| 🔌 **MCP Server** | `http://localhost:5748/mcp` | Streamable HTTP endpoint for AI agents (JSON-RPC) |
| ⚡ **REST API** | `http://localhost:5748` | Direct REST endpoints (`/search`, `/research`, `/health`) |
| 🗄️ **SurrealDB** | `http://localhost:7710` | Embedded/distributed graph database engine (`kavach` ns / `main` db) |

---

## 🖥️ Web Interface (GUI)

Access the Web UI at **`http://localhost:5750`**:

- **Instant Search & Autocomplete**: Real-time suggestion dropdown as you type.
- **Category Filtering**: Filter results by Tech, News, Science, Business, and Custom categories.
- **Live SSE Research Streaming**: Watch WebFind crawl seed URLs, discover links, and extract content live with progress tracking.
- **Rendered Content & Snippets**: Read extracted content directly with readability scores and highlighted query terms.

---

## 🤖 AI Agent Integration (MCP)

WebFind supports **MCP Streamable HTTP** (`http://localhost:5748/mcp`) and **Stdio** transports.

### 1. OpenCode (`~/.config/opencode/opencode.json`)
```json
{
  "mcp": {
    "webfind": {
      "type": "remote",
      "url": "http://localhost:5748/mcp",
      "enabled": true,
      "timeout": 120000
    }
  }
}
```

### 2. Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json`)
```json
{
  "mcpServers": {
    "webfind": {
      "url": "http://localhost:5748/mcp"
    }
  }
}
```

### 3. Claude Code / Cursor
```json
{
  "mcpServers": {
    "webfind": {
      "type": "url",
      "url": "http://localhost:5748/mcp"
    }
  }
}
```

---

## 🛠️ MCP Tool Reference

WebFind exposes 4 core MCP tools over Streamable HTTP and Stdio:

### 1. `webfind_search`
Search pages already indexed in the Tantivy engine.
- **Parameters**: `query` (string), `limit` (int), `hybrid` (bool), `include_graph` (bool), `timestamp` (string)
- **Features**: Fast local execution, optional dense vector re-ranking, PageRank weights, and query timestamp auto-enhancement.

### 2. `webfind_research`
Crawl live web pages starting from one or more seed URLs, index content on the fly, and return ranked search results.
- **Parameters**: `seed` (string), `query` (string), `depth` (int), `max_pages` (int), `topics` (string), `seeds` (string), `follow_external` (bool), `hybrid` (bool)
- **Features**: Live discovery, content-aware topic prioritization, cross-domain link following, and fresh evidence fetching.

### 3. `webfind_fetch`
Extract structured content, keywords, and links from single or multiple web pages without full site crawling.
- **Parameters**: `url` (string), `urls` (array of strings), `dynamic` (bool), `dynamic_wait_ms` (int), `extract_links` (bool), `extract_keywords` (bool)
- **Features**: Supports JavaScript rendering via headless Chromium fallback.

### 4. `webfind_graph`
Traverse link connections and examine the graph topology stored in SurrealDB.
- **Parameters**: `url` (string), `depth` (int), `direction` (`"inbound"`, `"outbound"`, or `"both"`)

---

## 💻 CLI Command Guide

All CLI subcommands can be run natively or inside the container via `docker exec`:

```bash
# Search existing Tantivy index with hybrid vector re-ranking
docker exec webfind-server webfind search "rust async runtime" --limit 10 --hybrid

# Deep research crawl starting from a seed URL
docker exec webfind-server webfind research --seed https://doc.rust-lang.org/book/ --query "ownership" --max-pages 20

# Crawl & index domain graph into SurrealDB
docker exec webfind-server webfind crawl --seed https://news.ycombinator.com --depth 3 --max-pages 100 --surreal-url ws://surrealdb:7710

# Fetch page content with headless Chromium for JS-heavy SPAs
docker exec webfind-server webfind fetch https://react.dev --dynamic --dynamic-wait-ms 3000

# Traverse link graph in SurrealDB
docker exec webfind-server webfind graph https://doc.rust-lang.org/ --depth 2

# Import from Common Crawl dataset
docker exec webfind-server webfind index import CC-MAIN-2026-01 --limit 50000

# Check engine and index status
docker exec webfind-server webfind status
```

---

## 🌐 REST API Endpoints

| Endpoint | Method | Query Parameters / Body | Description |
|---|---|---|---|
| `/health` | GET | - | Health status & index document count |
| `/search` | GET | `q`, `limit`, `hybrid` | Search local index |
| `/research` | GET | `seed`, `q`, `max_pages`, `depth` | Live crawl, index, and return ranked search |
| `/mcp` | POST | JSON-RPC 2.0 | MCP Streamable HTTP endpoint |
| `/api/web/suggest` | GET | `q` | Real-time autocomplete suggestions |
| `/api/web/categories` | GET | - | Retrieve domain categories & filter stats |
| `/api/web/research/stream` | GET | `seed`, `q`, `depth`, `max_pages` | Server-Sent Events (SSE) stream for live research |

---

## 🏗️ Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│                          Clients & Consumers                           │
│  ┌───────────────────────────────┐   ┌──────────────────────────────┐  │
│  │   AI Agent (MCP HTTP / Stdio) │   │   Human Web UI (HTMX + SSE)  │  │
│  └───────────────┬───────────────┘   └──────────────┬───────────────┘  │
└──────────────────┼──────────────────────────────────┼──────────────────┘
                   │ POST /mcp                        │ GET / HTTP
┌──────────────────▼──────────────────────────────────▼──────────────────┐
│                          WebFind Server (Axum)                         │
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│  ┌──────────────────────┐  ┌────────────────────┐  ┌────────────────┐  │
│  │   Crawler Engine     │  │  Headless Browser  │  │   Proxy Pool   │  │
│  │ (robots.txt, delay)  │  │ (Chromium / JS)    │  │ (CIDR / SOCKS) │  │
│  └──────────┬───────────┘  └─────────┬──────────┘  └────────────────┘  │
│             │                        │                                 │
│             ▼                        ▼                                 │
│  ┌──────────────────────────────────────────────┐                      │
│  │           Indexer (Tantivy Engine)           │                      │
│  │     BM25 Full-Text + FastEmbed HNSW Vector   │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │                                              │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │          Hybrid Ranker & Scorer              │                      │
│  │  (BM25 + Dense Vector + SurrealDB PageRank)  │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │ ws://surrealdb:7710                          │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │              SurrealDB Store                 │                      │
│  │    Link Graph Topology, PageRank, Audit Log  │                      │
│  └──────────────────────────────────────────────┘                      │
└────────────────────────────────────────────────────────────────────────┘
```

---

## ⚙️ Environment Variables

Configuration options for `compose.yml` or native deployments:

| Variable | Default | Description |
|---|---|---|
| `WEBFIND_DATA_DIR` | `/data` | Directory for Tantivy index and cache storage |
| `WEBFIND_GRAPH_STORE` | `surrealdb` | Graph backend (`surrealdb` or `memory`) |
| `WEBFIND_SURREAL_URL` | `ws://surrealdb:7710` | SurrealDB connection WebSocket URL |
| `WEBFIND_SURREAL_USER` | `root` | SurrealDB username |
| `WEBFIND_SURREAL_PASS` | `root` | SurrealDB password |
| `WEBFIND_SURREAL_NS` | `kavach` | SurrealDB namespace |
| `WEBFIND_SURREAL_DB` | `main` | SurrealDB database |
| `WEBFIND_GUI_PORT` | `4749` | Web UI HTTP server port |
| `WEBFIND_RATE_LIMIT` | Disabled | Per-IP rate limiting (requests per second) |

---

## 📜 License

[MIT](LICENSE)
