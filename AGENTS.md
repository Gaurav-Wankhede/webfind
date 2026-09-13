# WebFind — Agent Research Instructions

WebFind is a **pure CLI** system — there is no MCP server. Local models call the
`webfind` binary directly via a harness / system prompt. Every crawled record is
**persisted to the Turso graph store** (url_nodes + link_edges + page_content),
so each run builds durable graph-memory awareness that later searches reuse.
No temp JSON operations.

## Binary Location & System Location Awareness

WebFind is installed globally via `cargo install --path . --force && cargo clean`. The binary is available directly on the system `$PATH`:

- **macOS / Linux**: `~/.cargo/bin/webfind` (or simply `webfind`)
- **Windows**: `%USERPROFILE%\.cargo\bin\webfind.exe` (or `webfind.exe`)
- **Fallback Project Binary**: `target/release/webfind` (when working inside repo)

> **Agent Execution Rule**: Always invoke `webfind` directly as a first-class CLI tool. If running in an environment where `~/.cargo/bin` is not yet in the subshell PATH, resolve directly to `~/.cargo/bin/webfind` (UNIX) or `%USERPROFILE%\.cargo\bin\webfind.exe` (Windows).

## Why pure CLI (no MCP)

- **Single call, full implementation** — one command crawls, persists to the DB,
  indexes, ranks, and emits results. No repeated per-tool calls, no round-trip
  state, no server to keep alive.
- **No overall timeout** — research runs until done (deep mode deliberately
  longer). Only per-request HTTP timeouts apply (30s, 90s in deep mode).
- **Durable graph memory** — records live in the Turso database, not ephemeral
  JSON in the session. Future queries search the accumulated graph.
- **Reusable & context-friendly** — the result is a single JSON file on disk;
  nothing is streamed or accumulated into the session. Progress goes to stderr.

## Canonical command (for local model harnesses)

Run one CLI call, write the JSON result to a file, then **read that file with a
file-read tool**. Pure Rust end to end — no Python, no shell JSON parsing.

```sh
webfind research "your query here" \
  --max-pages 20 --delay 300 --deep --dynamic \
  --output /tmp/webfind_result.json
```

Then read `/tmp/webfind_result.json` and forward its contents. Records from this
run are already persisted to the graph store (`webfind.db`).

### Key research flags

| Flag | Default | Purpose |
|------|---------|---------|
| `--max-pages N` | 100 | Max pages to crawl |
| `--limit N` | 10 | Max results returned |
| `--include-content` | true | Return full scraped body text per result |
| `--dynamic` | false | Render JS-heavy / bot-protected pages via CDP Chromium |
| `--deep` | false | No crawl deadline/backoff caps + 90s request timeout + CDP stealth/scroll |
| `--seed URL` | auto | Explicit seed URL; auto-discovers from curated catalog otherwise |
| `--graph-store turso\|memory` | turso | Persist records to the embedded DB (graph memory) |
| `--turso-path PATH` | webfind.db | Database file for graph-memory persistence |
| `--output PATH` | stdout | Write the JSON result to a file (agent reads it with `Read`) |

### Other CLI commands (persist to the same DB)

- `webfind crawl --seed URL ...` — bulk crawl + persist records
- `webfind fetch URL --output json` — fetch a single URL
- `webfind search "query"` — search the accumulated index/graph
- `webfind graph URL` — traverse the persisted link graph
- `webfind status` — engine / index status

### Output contract

- **stdout / `--output` file** = a single JSON document. Fields: `query`,
  `total_results`, `results[]` (each with `rank`, `title`, `url`, `domain`,
  `excerpt`, `content`), `searched_at`.
- **stderr** = progress (`Researching: …`, `Crawl + persist complete …`), logs,
  and warnings.

## For AI agents

Do **one** CLI invocation per research task, write the result to `--output`,
read that file with your file-read tool, and forward the parsed JSON to the
caller. Do not call the tool repeatedly or spawn sub-agents for the same query.

Examples:

```sh
webfind research "Rust async runtime tokio 2026" --max-pages 8 --delay 300 --output /tmp/rust.json
```

```sh
webfind research "secure software supply chain practices 2026" --deep --dynamic --output /tmp/security.json
```
