# WebFind

> Self-hosted native search engine for AI agents — Docker, zero cost.

WebFind gives AI agents fresh, cited evidence instead of stale training data. Crawl, index, rank, and search the live web — then hand the results to your agent via MCP.

```
Agent → WebFind crawls seed → indexes pages → ranks results → returns cited JSON
```

## Why

LLM training data is frozen at cutoff. APIs change, versions ship, prices move, docs update. WebFind replaces "I think" with "here's the source."

## Quick Start

```bash
git clone https://github.com/gauravwankhede/WebFind.git
cd WebFind
docker compose up -d --build
```

Running on:
- **HTTP API**: `http://localhost:4747`
- **MCP Streamable HTTP**: `http://localhost:4747/mcp`
- **SurrealDB**: `ws://localhost:8000`

## How It Works

| What | When | Where |
|---|---|---|
| `search` | Query pages **already indexed** in SurrealDB | Fast, zero network calls |
| `research` | **Crawl live web** from a seed URL, index, then search | Fresh evidence on every call |

`search` = read from local index. `research` = crawl internet → index → search.

## CLI Commands

All commands run inside the container via `docker exec`:

```bash
# Search existing index
docker exec webfind-server webfind search --query "rust ownership" --limit 10

# Crawl + index + search
docker exec webfind-server webfind research --seed https://doc.rust-lang.org/book/ --query "ownership" --max-pages 10

# Fetch single URL
docker exec webfind-server webfind fetch https://docs.rs/tantivy/latest/tantivy/

# Traverse link graph
docker exec webfind-server webfind graph --url https://docs.rs/ --depth 2

# Index status
docker exec webfind-server webfind status
```

## MCP Tools

WebFind exposes 6 tools via MCP Streamable HTTP at `http://localhost:4747/mcp`.

### `webfind_search` — Query Existing Index

Search pages already indexed in SurrealDB. Zero network calls, instant results.

### `webfind_research` — Crawl Live Web + Index + Search

Crawls the live internet starting from a seed URL, indexes pages as it goes, then ranks and returns results. Fresh evidence on every call.

### `webfind_fetch` — Single URL Extraction

Fetch and extract content from a single URL. No crawling, no index.

### `webfind_fetch_parallel` — Multi-URL Extraction

Fetch multiple URLs in parallel. Same as `webfind_fetch` but concurrent.

### `webfind_research_parallel` — Multi-Seed Crawl

Run multiple research crawls in parallel. Crawls run concurrently; indexing is serialized.

### `webfind_graph` — Crawl Link Structure

Traverse the crawl link graph from a starting URL. Shows how pages connect.

## REST API

Direct HTTP endpoints (no MCP protocol):

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Health check + index size |
| `/search?q=...&limit=...` | GET | Search existing index |
| `/research?seed=...&q=...&max_pages=...` | GET | Crawl + index + search |
| `/mcp` | POST | MCP Streamable HTTP (JSON-RPC) |

## Agent Integration

### OpenCode

Add to `~/.config/opencode/opencode.json`:

```json
{
  "mcp": {
    "webfind": {
      "type": "remote",
      "url": "http://localhost:4747/mcp",
      "enabled": true,
      "timeout": 120000
    }
  }
}
```

### Claude Desktop

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "webfind": {
      "url": "http://localhost:4747/mcp"
    }
  }
}
```

### Claude Code

Add to your MCP settings:

```json
{
  "mcpServers": {
    "webfind": {
      "type": "url",
      "url": "http://localhost:4747/mcp"
    }
  }
}
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Agent (MCP)                       │
│         webfind_research / webfind_fetch             │
└──────────────────────┬──────────────────────────────┘
                       │ POST /mcp (JSON-RPC)
┌──────────────────────▼──────────────────────────────┐
│               WebFind Server (Docker)                 │
├─────────────────────────────────────────────────────┤
│                                                      │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ Crawler   │  │ Fetcher  │  │ Proxy Pool       │  │
│  │ (spider)  │  │ (reqwest)│  │ (HTTP/SOCKS5)   │  │
│  └─────┬────┘  └─────┬────┘  │ CIDR rotation    │  │
│        │             │       │ UA rotation       │  │
│        ▼             ▼       │ Session mgmt      │  │
│  ┌──────────────────────────┐└──────────────────┘  │
│  │      Indexer (Tantivy)   │                       │
│  │  BM25 + HNSW vectors     │                       │
│  └──────────┬───────────────┘                       │
│             │                                        │
│  ┌──────────▼───────────────┐                       │
│  │   Ranker (BM25 + PageRank│+ hybrid vector)       │
│  └──────────┬───────────────┘                       │
│             │ ws://surrealdb:8000                    │
│  ┌──────────▼───────────────┐                       │
│  │  Storage (SurrealDB)     │                       │
│  │  graph + PageRank cache   │                       │
│  └──────────────────────────┘                       │
└─────────────────────────────────────────────────────┘
```

## Configuration

All configuration via environment variables in `compose.yml`:

```yaml
services:
  webfind:
    environment:
      WEBFIND_DATA_DIR: /data
      WEBFIND_GRAPH_STORE: surrealdb
      WEBFIND_SURREAL_URL: ws://surrealdb:8000
      WEBFIND_SURREAL_USER: root
      WEBFIND_SURREAL_PASS: root
```

## Docker

```bash
docker compose up -d --build    # start
docker compose ps               # status
docker compose logs -f webfind  # logs
docker compose down             # stop
docker compose down -v          # stop + remove volumes
```

## License

MIT
