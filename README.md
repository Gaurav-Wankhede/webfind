<div align="center">

<img src="docs/assets/title.svg" alt="WebFind" width="740" />

**Self-hosted web research engine for free & local LLMs — single binary, zero cost, no cloud, no API keys.**

[![Rust 1.98+](https://img.shields.io/badge/rust-1.98%2B-blue.svg?logo=rust&style=flat-square)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg?style=flat-square)](LICENSE)
[![Platform: Linux | macOS | Windows](https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg?style=flat-square)](#quick-start--installation)
[![Architecture: Pure CLI + Axum Web UI](https://img.shields.io/badge/architecture-Pure%20CLI%20%2B%20Web%20UI-cyan.svg?style=flat-square)](#architecture)

<p align="center">
  <a href="#key-features">Features</a> •
  <a href="#prerequisites">Prerequisites</a> •
  <a href="#installing-rust">Install Rust</a> •
  <a href="#quick-start--installation">Installation</a> •
  <a href="#ai-agent-integration-pure-cli">AI Integration</a> •
  <a href="#cli-command-guide">CLI Guide</a> •
  <a href="#architecture">Architecture</a>
</p>

</div>

---

## Why WebFind?

When you use hosted frontier AI models (Claude, Codex, ChatGPT Pro), web search is tightly integrated into the provider harness. But on **free and locally-hosted models** (Ollama, DeepSeek, Qwen, Mistral, Llama), live web search is gated or absent — degrading model answers with stale hallucinated knowledge.

**WebFind fills that gap with zero external dependencies:**
- **Single-Call AI Harness Protocol:** One command crawls, persists to an embedded graph store, indexes, ranks, and returns grounded cited results in JSON.
- **Embedded Turso/libSQL Memory:** Crawled records persist in a durable local graph (`url_nodes` + `link_edges` + `page_content`). Future searches reuse the accumulated graph.
- **Hybrid Retrieval:** BM25 full-text search combined with local dense vector embeddings (`fastembed-rs`) and PageRank fused via Reciprocal Rank Fusion (RRF).
- **Dual Personality:** Operates as a blazing-fast **pure CLI** for autonomous coding agents and provides a **Google-style Web Interface (GUI)** with live Server-Sent Events (SSE) crawl streaming for humans.

```
Local Model (Agent Harness) -> webfind research (single CLI call) -> Crawl & Turso Graph Store -> Hybrid BM25 + Vector + PageRank -> Grounded JSON Context
```

---

## Key Features

- **Google-Style Web Interface**: Fast, responsive Web UI built with Axum, HTMX, Tailwind CSS, real-time SSE research streaming, auto-complete search suggestions, and domain category filtering.
- **Pure CLI (Zero-MCP Overhead)**: One command executes end-to-end research. No background daemon to babysit, no complex MCP handshakes, and zero round-trip protocol latency.
- **Durable Graph Memory**: Every crawl is persisted directly into embedded Turso/libSQL. No ephemeral temp JSON files — research compounds into a local knowledge base.
- **Hybrid BM25 + Dense Vector Search**: High-performance lexical search merged with local vector embeddings (`fastembed-rs`) and graph PageRank via Reciprocal Rank Fusion (RRF).
- **High-Concurrency Web Crawler**: Full `robots.txt` compliance, domain session stickiness, adaptive rate limiting, User-Agent rotation, proxy CIDR pool routing, and headless Chromium (CDP) for dynamic SPAs.
- **Uncapped Deep Mode (`--deep`)**: Removes crawl deadlines and backoff caps (with 90s request timeouts) to let comprehensive multi-page investigations finish reliably.
- **High-Density Noise Stripping**: Filters scripts, styles, advertisements, tracking, navigation bars, and footers to pass token-efficient, high-signal Markdown extracts to your LLM.
- **Zero Cloud & Zero Cost**: Self-hosted on your machine. No monthly subscriptions, no rate-limited search APIs, and optional AES-256-CBC encryption at rest.

---

## Architecture

WebFind bridges local AI agents and human research through a unified, high-performance Rust core:

<p align="center">
  <img src="docs/assets/architecture.svg" alt="WebFind Architecture Diagram" width="100%" />
</p>

---

## Prerequisites

Before installing and compiling WebFind from source, verify that your machine has the following dependencies installed:

| Prerequisite | Minimum Version | Required For | Verification Command |
|---|---|---|---|
| **Rust toolchain** | `1.98.0+` (Rust 2024 Edition) | Building the `webfind` binary & native dependencies | `rustc --version` |
| **C / C++ Compiler & Linker** | `clang` / `gcc` / MSVC | Compiling `libsql-ffi` and native C libraries | `cc --version` (or `clang --version`) |
| **Git** | Any modern version | Cloning the repository | `git --version` |
| **Chromium** *(Optional)* | Any modern release | Dynamic JS rendering / SPA scraping (`--dynamic`) | `which chromium || which google-chrome` |
| **Docker** *(Optional)* | 20.10+ | Running the Web UI and HTTP daemon via containers | `docker --version` |

---

## Installing Rust

WebFind utilizes modern Rust features (Edition 2024, Rust 1.98+). Follow the instructions below for your operating system to set up or update your Rust environment:

### macOS and Linux

Install Rust using the official `rustup` installer:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the on-screen instructions (Option `1` - default installation). Once installed, refresh your shell profile:

```bash
source "$HOME/.cargo/env"
```

Verify that Rust and Cargo are available:

```bash
rustc --version
cargo --version
```

If you already have Rust installed, update to the latest stable toolchain:

```bash
rustup update stable
```

### Windows

1. Download and run **`rustup-init.exe`** from [rustup.rs](https://rustup.rs).
2. If prompted, install the **Microsoft C++ Build Tools** (Visual Studio Installer with the "Desktop development with C++" workload).
3. Restart your PowerShell or Command Prompt window and verify:

```powershell
rustc --version
cargo --version
```

---

## Quick Start & Installation

Install `webfind` globally across **Linux, macOS, and Windows**. This compiles the release binary directly into your system's global Cargo bin directory:

```bash
git clone https://github.com/Gaurav-Wankhede/WebFind.git
cd WebFind
cargo install --path . --force && cargo clean
```

> **Universal Global Binary Locations (Already in your `$PATH`):**
> - **macOS / Linux**: `~/.cargo/bin/webfind`
> - **Windows**: `%USERPROFILE%\.cargo\bin\webfind.exe`
>
> Once installed, you can invoke `webfind` from any directory or terminal session without navigating to the project folder.

### Run Your First Research Query

Run a deep research crawl in a single call. WebFind crawls seed URLs, indexes content, ranks with BM25 + Vector + PageRank, persists the graph to `webfind.db`, and outputs structured JSON:

```bash
webfind research "Rust async runtime tokio" \
  --max-pages 20 --delay 300 --deep --dynamic \
  --output /tmp/webfind_result.json
```

Inspect `/tmp/webfind_result.json` — your local model harness can read this file directly to ground its answers.

---

## Web Interface & Docker Deployment

For human use, WebFind includes a modern Google-style Web Interface and REST API. You can launch the complete service using Docker Compose:

```bash
docker compose up -d --build
```

### Endpoints & Services

| Interface | URL | Description |
|---|---|---|
| **Web UI** | `http://localhost:5750` | Search interface with autocomplete, category filters, and live SSE crawl streaming |
| **REST API** | `http://localhost:5748` | REST API endpoints for remote ingestion (`/search`, `/research`, `/health`) |

All persistent data (Turso DB, link graph, cache) resides in the `./data` volume. Backing up the database is as simple as:

```bash
cp webfind.db backup.db
```

---

## AI Agent Integration (Pure CLI)

WebFind is intentionally architected as a **pure CLI system** — avoiding the brittle state machines, connection drops, and memory leaks of persistent MCP servers. Local agent harnesses call the `webfind` binary directly.

### The Single-Call Pattern

```bash
webfind research "your query here" \
  --max-pages 20 --delay 300 --deep --dynamic \
  --output /tmp/webfind_result.json
```

- **stdout / `--output` file**: Returns a clean JSON document containing `query`, `total_results`, `results[]` (with `rank`, `title`, `url`, `domain`, `excerpt`, and sanitized `content`), and timestamp.
- **stderr**: Real-time progress indicators (`Researching: ...`, `Crawl + persist complete ...`), diagnostics, and crawl telemetry.
- **Graph Awareness**: All discovered URLs and pages persist to `webfind.db`. Future searches automatically leverage past crawls.

### Essential Flags

| Flag | Default | Description |
|---|---|---|
| `--max-pages N` | `100` | Maximum pages to crawl in this run |
| `--limit N` | `10` | Maximum ranked search results to return |
| `--include-content` | `true` | Include sanitized full markdown/text content per result |
| `--dynamic` | `false` | Enable headless Chromium (CDP) for JavaScript-rendered SPAs |
| `--deep` | `false` | Uncapped mode: disables timeouts/backoff caps for exhaustive crawls |
| `--seed URL` | `auto` | Explicit seed URL (auto-discovers from catalog if omitted) |
| `--graph-store turso\|memory` | `turso` | Persistence backend (`turso` for durable graph or `memory`) |
| `--turso-path PATH` | `webfind.db` | Target path for the embedded Turso/libSQL database file |
| `--output PATH` | `stdout` | Target file path for the formatted JSON results |

---

## CLI Command Guide

```bash
# Deep research: crawl + persist to graph + search, output structured JSON
webfind research "rust async runtime" --max-pages 20 --deep --dynamic --output /tmp/r.json

# Search the existing accumulated local graph and full-text index
webfind search "tokio channels" --limit 10 --hybrid

# Crawl and persist a domain graph into the embedded Turso store
webfind crawl --seed https://news.ycombinator.com --depth 3 --max-pages 100

# Fetch and extract clean content from a JavaScript-heavy SPA
webfind fetch https://react.dev --dynamic --dynamic-wait-ms 3000

# Inspect the link graph in Turso for a given domain or URL
webfind graph https://doc.rust-lang.org/ --depth 2

# Check database health, total indexed documents, and engine status
webfind status
```

---

## REST API Endpoints

When running in HTTP / server mode, WebFind exposes high-performance REST and streaming endpoints:

| Endpoint | Method | Parameters | Description |
|---|---|---|---|
| `/health` | `GET` | - | Health status and indexed document count |
| `/search` | `GET` | `q`, `limit`, `hybrid` | Query local full-text & vector index |
| `/research` | `GET` | `seed`, `q`, `max_pages`, `depth` | Execute live crawl, index, and return ranked results |
| `/api/web/suggest` | `GET` | `q` | Real-time autocomplete suggestions |
| `/api/web/categories` | `GET` | - | Retrieve domain categories and index stats |
| `/api/web/research/stream` | `GET` | `seed`, `q`, `depth`, `max_pages` | Server-Sent Events (SSE) stream for live crawl visualization |

---

## Configuration & Environment Variables

| Variable | Default | Description |
|---|---|---|
| `WEBFIND_DATA_DIR` | Current directory | Root directory for Turso database and local cache |
| `WEBFIND_GRAPH_STORE` | `turso` | Graph memory store (`turso` or `memory`) |
| `WEBFIND_TURSO_PATH` | `webfind.db` | Embedded Turso/libSQL database file location |
| `WEBFIND_GUI_PORT` | `4749` | Web UI and HTTP server listening port |
| `WEBFIND_RATE_LIMIT` | `60` | Per-IP rate limiting (requests per second; `0` disables) |
| `WEBFIND_BODY_LIMIT` | `1048576` | Maximum request body size in bytes (1MB) |
| `WEBFIND_CORS_ORIGINS` | *(empty)* | Comma-separated allowed CORS origins |
| `WEBFIND_CONFIG` | `./webfind.toml` | Custom configuration file path |

---

## License

WebFind is open-source software licensed under the [MIT License](LICENSE).
