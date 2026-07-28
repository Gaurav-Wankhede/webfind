# WebFind MCP Integration for Agentic CLIs

> Docker-only. WebFind runs as a Streamable HTTP MCP server on port 4747.

## What the MCP server exposes

Start the server with:

```bash
docker compose up -d --build
```

It exposes 6 tools via MCP Streamable HTTP at `http://localhost:4747/mcp`:

- `webfind_search` — search the existing local index (SurrealDB).
- `webfind_research` — crawl a seed URL, index the pages, then search them.
- `webfind_fetch` — fetch and extract content from a single URL.
- `webfind_fetch_parallel` — fetch multiple URLs in parallel.
- `webfind_research_parallel` — run multiple research crawls in parallel.
- `webfind_graph` — traverse the stored crawl graph.

## search vs research

- `search` queries pages **already indexed** in SurrealDB. Fast, zero network calls.
- `research` **crawls the live internet** from a seed URL, indexes pages, then searches. Fresh evidence on every call.

## Running with Docker + SurrealDB

```bash
cd /Users/gauravwankhede/WebFind
docker compose up -d --build
```

This starts:
- `webfind-server` on `http://localhost:4747`
- `webfind-surrealdb` on `http://localhost:7790`

Both services are configured with `restart: unless-stopped` and a healthcheck so SurrealDB is ready before WebFind connects.

## MCP endpoint

```
POST http://localhost:4747/mcp
Content-Type: application/json
Accept: application/json, text/event-stream
```

The MCP Streamable HTTP protocol requires the `Accept` header to include both `application/json` and `text/event-stream`.

## OpenCode

Edit `~/.config/opencode/opencode.json`:

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

## Claude Desktop

Edit your Claude Desktop MCP config:

- **macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Windows**: `%APPDATA%\Claude\claude_desktop_config.json`
- **Linux**: `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "webfind": {
      "url": "http://localhost:4747/mcp"
    }
  }
}
```

## Claude Code

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

## Other MCP-compatible agents

Any client that supports the MCP Streamable HTTP transport can use:

```
POST http://localhost:4747/mcp
```

Adapt the surrounding config envelope to your client's expected JSON shape.

## Tool schemas

### `webfind_search`

```json
{
  "query": "search query string",
  "limit": 10,
  "hybrid": false,
  "include_graph": false
}
```

### `webfind_research`

```json
{
  "seed": "https://example.com/",
  "query": "search query",
  "max_pages": 50,
  "limit": 10,
  "delay": 1000,
  "hybrid": false,
  "include_content": false,
  "include_graph": false
}
```

### `webfind_fetch`

```json
{
  "url": "https://example.com/page",
  "extract_links": false,
  "extract_keywords": false,
  "proxies": null,
  "dynamic": null,
  "dynamic_wait_ms": null
}
```

### `webfind_fetch_parallel`

```json
{
  "urls": ["https://example.com/a", "https://example.com/b"],
  "extract_links": false,
  "extract_keywords": false,
  "proxies": null,
  "dynamic": null,
  "dynamic_wait_ms": null
}
```

### `webfind_research_parallel`

```json
{
  "jobs": [
    {"seed": "https://example.com/a", "query": "topic A"},
    {"seed": "https://example.com/b", "query": "topic B"}
  ]
}
```

### `webfind_graph`

```json
{
  "url": "https://example.com/",
  "depth": 1,
  "direction": "both"
}
```

## Verification

1. `docker compose up -d --build`
2. `curl http://localhost:4747/health` — should return `{"status":"ok","version":"0.1.0","index_size":...}`
3. Restart the agent / editor.
4. Ask the agent to run a research or search call. You should see all 6 tools in the tool list.

## Troubleshooting

- **Tool not appearing?** Check that `http://localhost:4747/mcp` is reachable from the agent. The Docker container must be running.
- **Timeouts?** Increase `timeout` in the MCP config or reduce `max_pages`.
- **No results?** For `search`: confirm pages are indexed (`docker exec webfind-server webfind status`). For `research`: confirm the seed URL is reachable and `robots.txt` allows crawling.
- **SurrealDB connection errors at startup?** Transient — SurrealDB may not be ready yet. The server still starts and functions. Restart the container if needed: `docker compose restart webfind`.
