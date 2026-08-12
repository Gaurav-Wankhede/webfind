# WebFind

> Self-hosted web research for free & local LLMs — single binary, zero cost, no cloud, no auth.

WebFind gives **free and locally-hosted models** the live web research that paid plans gate behind web search. When you use a paid model (Claude, Codex), web search is built into the harness. But on **free / local models** in OpenCode, Claude Code, Codex, Pi Agent, or LM Studio, web search doesn't work — that's exactly the gap WebFind fills.

WebFind crawls, indexes, ranks, and searches the live web, then hands fresh cited results to your model over **MCP stdio** or the **CLI**. Everything runs on your machine: no API keys, no cloud, no authentication.

```
Local Model (MCP stdio / CLI) → WebFind Crawl & Discovery → BM25 + DiskANN Vector + Turso Link Graph → Ranked Cited Results → back to your model
```

---

## 🌟 Key Features

- 🌐 **Google-Style Web Interface**: Modern, responsive Web UI built with Axum, HTMX, Tailwind CSS, real-time SSE research streaming, auto-complete search suggestions, and category filtering.
- 🤖 **MCP Native (Streamable HTTP & Stdio)**: Seamless integration with OpenCode, Claude Desktop, Claude Code, Cursor, and any MCP-compatible agent platform. Optional OAuth 2.1 + PKCE for multi-tenant deployments.
- ⚡ **Hybrid BM25 + Dense Vector Search**: High-performance full-text search combined with dense vector embeddings (`fastembed-rs`) for semantic search re-ranking, fused with Reciprocal Rank Fusion.
- 🕸️ **Deep Web Crawler**: High-concurrency crawler with `robots.txt` compliance, rate limiting, domain session stickiness, User-Agent rotation, proxy CIDR pool routing, and headless Chromium fallback for JavaScript-rendered SPA pages.
- 📊 **Turso Embedded Graph & PageRank**: Single-file SQLite (Turso/libSQL) for the link graph, vector index (DiskANN), full-text index (FTS5), and PageRank — zero external database.
- 🎯 **Clean Noise-Free Extraction for AI Agents**: Strips out scripts, styles, ads, navigation bars, and headers/footers. Focuses strictly on core structural elements (`<h1-h6>`, `<p>`, `<div>`, `<span>`) to deliver high-density, token-efficient Markdown content to LLM context windows (inspired by Firecrawl).
- 📦 **Common Crawl Import**: Tools to import and index massive datasets directly from Common Crawl (`CC-MAIN`).
- 🔒 **Encryption at Rest**: Optional SQLCipher AES-256-CBC encryption for the Turso database file.

---

## 🚀 Quick Start (Docker)

Spin up the fully self-contained WebFind service with Docker Compose — no separate database, no schema init:

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

All data (Turso DB, Tantivy index, cache) lives in the `/data` volume. Backup is a file copy: `cp webfind.db backup.db`.

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

### Free / local models (Stdio) — WebFind's primary purpose

Paid plans bundle web search into the harness. **Free and local models do not get it** — that's why WebFind exists: it adds live web research to any free model via the **stdio** MCP transport (WebFind's default).

Run the stdio server, then register it in your harness. Examples:

**OpenCode** (`~/.config/opencode/opencode.json`):
```json
{
  "mcp": {
    "webfind": {
      "type": "stdio",
      "command": "webfind",
      "args": ["serve", "--transport", "stdio"],
      "enabled": true
    }
  }
}
```

**Claude Code / Codex / Pi Agent** (`~/.claude.json`, `~/.codex/config.toml`, or the equivalent MCP config for your tool):
```json
{
  "mcpServers": {
    "webfind": {
      "command": "webfind",
      "args": ["serve", "--transport", "stdio"]
    }
  }
}
```

**LM Studio / any MCP client** — point it at the `webfind` binary with the same stdio args, or run the server yourself and connect:
```bash
webfind serve --transport stdio
```

Once wired, your free/local model can ask WebFind for the latest solutions — WebFind fetches the live web and returns fresh, cited results that were never in the model's training data. No API keys, no cloud, no auth; everything stays on your machine.

### Remote / agent-host (HTTP)

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
Traverse link connections and examine the graph topology stored in the Turso knowledge graph.
- **Parameters**: `url` (string), `depth` (int), `direction` (`"inbound"`, `"outbound"`, or `"both"`)

---

## 💻 CLI Command Guide

All CLI subcommands can be run natively or inside the container via `docker exec`:

```bash
# Search existing index with hybrid vector re-ranking
docker exec webfind-server webfind search "rust async runtime" --limit 10 --hybrid

# Deep research crawl starting from a seed URL
docker exec webfind-server webfind research --seed https://doc.rust-lang.org/book/ --query "ownership" --max-pages 20

# Crawl & index domain graph into the embedded Turso store
docker exec webfind-server webfind crawl --seed https://news.ycombinator.com --depth 3 --max-pages 100

# Fetch page content with headless Chromium for JS-heavy SPAs
docker exec webfind-server webfind fetch https://react.dev --dynamic --dynamic-wait-ms 3000

# Traverse the link graph in Turso
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
                   │ POST /mcp (OAuth 2.1 optional)   │ GET / HTTP
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
│  │          Indexer (BM25 + Vector)             │                      │
│  │     Tantivy BM25 + FastEmbed / DiskANN       │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │                                              │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │          Hybrid Ranker & Scorer (RRF)        │                      │
│  │      (BM25 + Dense Vector + PageRank)        │                      │
│  └──────────────────────┬───────────────────────┘                      │
│                         │ embedded Turso (webfind.db)                  │
│  ┌──────────────────────▼───────────────────────┐                      │
│  │              Turso / libSQL Store            │                      │
│  │   Link Graph, FTS5, DiskANN Vector, PageRank │                      │
│  └──────────────────────────────────────────────┘                      │
└────────────────────────────────────────────────────────────────────────┘
```

---

## ⚙️ Environment Variables

Configuration options for `compose.yml` or native deployments:

| Variable | Default | Description |
|---|---|---|
| `WEBFIND_DATA_DIR` | cwd | Directory for the Turso DB, Tantivy index, and cache storage |
| `WEBFIND_GRAPH_STORE` | `turso` | Graph backend (`turso` or `memory`) |
| `WEBFIND_TURSO_PATH` | `webfind.db` | Path to the embedded Turso/libSQL database file |
| `WEBFIND_GUI_PORT` | `4749` | Web UI HTTP server port |
| `WEBFIND_RATE_LIMIT` | `60` | Per-IP rate limiting (requests per second; `0` disables) |
| `WEBFIND_BODY_LIMIT` | `1048576` | Max request body size in bytes (API); MCP uses 256KB |
| `WEBFIND_CORS_ORIGINS` | *(empty)* | Comma-separated allowed origins (empty = restrictive, no CORS) |
| `WEBFIND_MCP_ALLOWED_HOSTS` | `localhost` | Comma-separated allowed `Host:` headers for the MCP endpoint (anti-DNS-rebinding) |
| `WEBFIND_AUTH_MODE` | `off` | `oauth2.1` enables OAuth 2.1 + PKCE on the MCP endpoint; `off` = single-tenant |
| `WEBFIND_OAUTH_ISSUER` | - | OAuth issuer URL (e.g. `https://auth.example.com`) |
| `WEBFIND_OAUTH_CLIENT_ID` | - | OAuth client ID |
| `WEBFIND_OAUTH_CLIENT_SECRET` | - | OAuth client secret (confidential clients) |
| `WEBFIND_OAUTH_REDIRECT_URI` | - | OAuth redirect/callback URI |
| `WEBFIND_OAUTH_SCOPES` | - | Comma-separated OAuth scopes |
| `WEBFIND_OAUTH_JWKS_URI` | - | JWKS URI for token validation (defaults to issuer) |
| `WEBFIND_OAUTH_AUDIENCE` | - | OAuth token audience |
| `WEBFIND_OAUTH_AUDIT_LOG` | - | Path to append FR-11 audit-log entries (JSONL) |

---

## 📜 License

[MIT](LICENSE)
