# Product Requirements Document (PRD): Engine-First Autonomous Research Pipeline

**Document Version:** 1.0.0  
**Status:** Approved for Implementation  
**Target Component:** WebFind Research & Crawl Engine (`webfind research`, `webfind crawl`)  
**Target Repository:** `Gaurav-Wankhede/webfind`  

---

## 1. Executive Summary & Problem Statement

### 1.1 The Problem
In existing agentic web research systems, crawlers frequently rely on direct seed URLs supplied by users or speculated by AI agents. This introduces critical vulnerabilities and inefficiencies:
1. **Malicious / Fabricated URL Injection (SSRF & Cache Poisoning):** Speculative or malicious seed inputs (e.g., internal subnets, AWS metadata endpoints `169.254.169.254`, loopback addresses, or non-HTTP schemes) can be used to probe private infrastructure or poison persistent graph memory.
2. **False Positives (Hallucinated Seeds):** LLMs frequently hallucinate dead links or non-existent URLs (e.g., `https://<keyword>.com` or outdated paths), leading to DNS failures, connection timeouts, and wasted crawler latency budgets.
3. **False Negatives (Over-Restricted Domain Crawls):** Rigid domain boundary rules miss critical multi-domain documentation (e.g., searching on `crates.io` when official reference guides reside on `docs.rs` or `github.com`).
4. **Cost & API Friction:** Many search services depend on paid, metered APIs (Google Custom Search, Bing Search API, Tavily, Brave Search API) requiring keys, rate-limit management, and recurring monthly expenses.

### 1.2 The Solution
Refactor WebFind into a strict **Engine-First, Zero-Cost, Pure-CLI Autonomous Pipeline**:
$$\text{Key-Free Engines} \longrightarrow \text{Security Gate} \longrightarrow \text{Crawl} \longrightarrow \text{Scrape} \longrightarrow \text{Async Index/Score} \longrightarrow \text{Immediate Final Output}$$

The pipeline will exclusively use **free, key-less search engine adapters** (DuckDuckGo Lite, Bing SERP, Mojeek, Marginalia, Wikipedia, HackerNews, Lobsters, MDN, Crates.io, arXiv, StackOverflow) combined with Reciprocal Rank Fusion (RRF) to ground all crawls in live, verified, consensus web links. Heavy indexing and persistence tasks happen asynchronously in background workers, decoupling output generation from database storage to maximize agent speed.

---

## 2. Goals & Success Metrics

### 2.1 Core Goals
- **100% Key-Free / Zero-Cost:** Zero external paid API dependencies or credentials required.
- **SSRF & Injection Immunity:** Complete network-level protection against loopback, private RFC 1918, and link-local IP resolution before any socket connects.
- **Zero Hallucinated Seeds:** All automatic research crawls originate from verified consensus SERPs.
- **Sub-Second to Low-Latency Agent Response:** Emitting structured results directly to stdout or disk without waiting for vector embedders or SQLite/Turso graph commits to finish.

### 2.2 Success Metrics (SLOs & Invariants)
| Metric | Current Baseline | Target Post-Refinement |
| :--- | :--- | :--- |
| **API Cost per Query** | $0.00 | **$0.00 (Hard invariant)** |
| **Seed Discovery Consensus** | Single engine / fallback | **$\ge 2$ Engine Agreement or authoritative vertical hit** |
| **Private IP Leakage / SSRF** | Vulnerable if unchecked URL passed | **0% (100% blocked at DNS pre-flight gate)** |
| **P90 Output Latency** | ~5–12s (sequential processing) | **$\le 2.5\text{s}$ (via async decoupling)** |
| **Corrupted Graph Entries** | Occasional invalid status nodes | **0 invalid nodes written to Turso** |

---

## 3. Detailed Architecture & Pipeline Stages

```
             ┌──────────────────────────────────────────────┐
             │       User Query / AI Agent Prompt           │
             └──────────────────────┬───────────────────────┘
                                    │
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 1: Key-Free Engine Fan-Out & Consensus Discovery                        │
│ - Parallel queries to 11 key-free engine adapters (tokio::join!)              │
│ - Per-engine timeout: 10s; global budget: 20s                                 │
│ - Multi-signal normalization & Reciprocal Rank Fusion (RRF, k=60)             │
└───────────────────────────────────┬───────────────────────────────────────────┘
                                    │ Unsanitized candidate URLs
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 2: Seed Ingress Security Gate (Anti-SSRF & Anti-Poisoning)              │
│ - Scheme Whitelist: http:// and https:// only                                │
│ - DNS Pre-flight Check: Resolve IP and filter RFC 1918, 127.0.0.0/8, 169.254  │
│ - Canonicalization: Normalize tracking params, trailing slashes, fragments    │
│ - De-duplication: Top-N consensus selection (Default: 3–5 seeds)              │
└───────────────────────────────────┬───────────────────────────────────────────┘
                                    │ Validated, live public seeds
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 3: High-Throughput Scoped Crawler                                       │
│ - Auto-depth discovery using site-declared llms.txt and sitemap.xml           │
│ - Cross-domain topic following for related documentation                      │
│ - Headless CDP Chromium stealth mode for dynamic/bot-protected targets        │
│ - Zero-copy HTTP response streaming                                           │
└───────────────────────────────────┬───────────────────────────────────────────┘
                                    │ Raw document streams
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 4: Readability Scraping & Metadata Extraction                           │
│ - Mozilla Readability (Rust port): Boilerplate, nav, ad, cookie stripping     │
│ - Reading ease (Flesch-Kincaid), word count, language identification          │
│ - Entity recognition (emails, handles, prices, dates) & TF-IDF key terms      │
└───────────────────────────────────┬───────────────────────────────────────────┘
                                    │ Clean StructuredContent documents
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 5: Synchronous Ranking & Immediate Final Output                         │
│ - BM25 score calculation + Content quality multiplier                         │
│ - Top-K document selection (Default: 10 results)                              │
│ - Immediate CLI emit to stdout or --output file                               │
└───────────────────────────────────┬───────────────────────────────────────────┘
                                    │ Non-blocking MPSC channel handoff
                                    ▼
┌───────────────────────────────────────────────────────────────────────────────┐
│ Stage 6: Asynchronous Background Persistence (Behind the Scenes)              │
│ - Turso / libSQL graph memory: url_nodes, link_edges, page_content            │
│ - FastEmbed dense vector embeddings (ONNX)                                    │
│ - In-memory inverted index refresh                                            │
│ - Runs isolated on tokio background pool without blocking CLI process         │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Technical Specifications & Functional Requirements

### 4.1 Stage 1: Key-Free Engine Fan-Out (`src/engine/web_index/`)
- **Query Router:** Intelligently partition queries across generic and vertical engines.
  - *Generic Engines:* DuckDuckGo Lite (HTML), Bing (SERP scraper), Mojeek, Marginalia.
  - *Technical/Vertical Engines:* Crates.io (crates/libraries), MDN (web APIs), StackOverflow, Lobsters, HackerNews, arXiv, Wikipedia.
- **Fail-Safe Operation:** If any engine fails or returns a bot challenge, log a trace warning and continue with remaining responders. Never abort the search due to partial engine failure.
- **RRF Algorithm:**
  $$RRF\_Score(d) = \sum_{e \in Engines} \frac{1}{k + Rank_e(d)} \quad \text{where } k = 60$$

### 4.2 Stage 2: Ingress Security Gate (`src/engine/security_gate.rs`)
- **Scheme Validation:** Reject any URL not starting with `http://` or `https://`.
- **DNS / IP Pre-flight Resolution:**
  - Resolve domain to `std::net::IpAddr`.
  - Fail-closed if IP belongs to:
    - Loopback: `127.0.0.0/8`, `::1`
    - Private IPv4: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
    - Link-Local & Cloud Metadata: `169.254.0.0/16`, `fe80::/10`
    - Broadcast / Unspecified: `0.0.0.0`, `255.255.255.255`
- **Normalization:** Strip tracking parameters (`utm_*`, `fbclid`, `gclid`, `ref`), lower-case hostnames, and remove URL fragments.

### 4.3 Stage 3 & 4: Scoped Crawl & Scrape Pipeline (`src/engine/bulk_crawler.rs`, `fetcher.rs`)
- **Politeness & Rate-Limiting:** Maintain token-bucket rate limits per domain (default: 1 req/s, burst 3).
- **Stealth & UA Rotation:** Rotate realistic desktop user-agents for every outbound request.
- **Automatic Fallback:** Plain HTTP request first; if the returned body is an empty JS shell or bot challenge, trigger the headless CDP Chromium renderer with stealth scripts.
- **Readability & Content Validation:** Enforce `MIN_VALID_WORDS = 5` to filter blank pages while preserving short documentation landing pages.

### 4.4 Stage 5 & 6: Decoupled Scoring and Background Persistence
- **Immediate Agent Hand-off:**
  - The CLI thread must rank pages using in-memory BM25 + quality metrics and immediately serialize the output to `--output <path>` or stdout.
- **Background Worker:**
  - A detached background task consumes scraped content via an unbounded or high-capacity MPSC queue (`tokio::sync::mpsc::channel`).
  - Writes to embedded libSQL (`webfind.db`) and computes ONNX dense embeddings asynchronously.
  - If the CLI command terminates, ensure graceful flush or resilient write-ahead logging (WAL) in SQLite.

---

## 5. Input and Output Data Contracts

### 5.1 CLI Input Command
```bash
webfind research "<query>" \
  [--max-pages <N>] \
  [--limit <N>] \
  [--delay <MS>] \
  [--deep] \
  [--dynamic] \
  [--output <PATH>]
```
*(Notice: `--seed` is completely optional. When omitted, the engine-first consensus discovery initiates automatically.)*

### 5.2 Agent Output JSON Schema (`output.json`)
```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "WebFindResearchResult",
  "type": "object",
  "required": ["query", "total_results", "returned", "latency_ms", "results", "metadata"],
  "properties": {
    "query": { "type": "string" },
    "depth": { "type": "string", "enum": ["standard", "deep"] },
    "total_results": { "type": "integer" },
    "returned": { "type": "integer" },
    "latency_ms": { "type": "integer" },
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["rank", "url", "title", "snippet", "domain", "score", "scores", "content"],
        "properties": {
          "rank": { "type": "integer" },
          "url": { "type": "string", "format": "uri" },
          "title": { "type": "string" },
          "snippet": { "type": "string" },
          "domain": { "type": "string" },
          "score": { "type": "number" },
          "scores": {
            "type": "object",
            "properties": {
              "bm25": { "type": "number" },
              "vector": { "type": ["number", "null"] },
              "graph": { "type": "number" },
              "freshness": { "type": "number" },
              "quality": { "type": "number" },
              "final_score": { "type": "number" }
            }
          },
          "content": {
            "type": "object",
            "required": ["text", "word_count"],
            "properties": {
              "text": { "type": "string" },
              "excerpt": { "type": "string" },
              "word_count": { "type": "integer" },
              "reading_time_seconds": { "type": "integer" },
              "markdown": { "type": "string" }
            }
          }
        }
      }
    },
    "metadata": {
      "type": "object",
      "required": ["engine_version", "discovery_sources"],
      "properties": {
        "engine_version": { "type": "string" },
        "discovery_sources": { "type": "array", "items": { "type": "string" } },
        "index_size": { "type": "integer" }
      }
    }
  }
}
```

---

## 6. Implementation Phasing & Work Breakdown

### Phase 1: Security Gate & SSRF Hardening
- Implement `src/engine/security_gate.rs` with IP resolution and network range blacklisting.
- Integrate validation into both discovery hits and user-supplied seeds.

### Phase 2: Refine Key-Free Engine Adapters & Query Router
- Audit all 11 adapters in `src/engine/web_index/engines/`.
- Ensure robust error handling when individual engines return captchas or non-200 responses.
- Implement domain-specific routing (e.g., tech queries route to MDN/Crates.io/GitHub).

### Phase 3: Decoupled Background Persistence
- Refactor `src/engine/research_service.rs` so that writing to `webfind.db` and ONNX vector embedding runs in a detached task.
- Ensure the primary CLI flow renders/writes the output file as soon as the in-memory documents are ranked.

### Phase 4: Verification & E2E Validation
- Add integration tests for:
  1. Rejecting `127.0.0.1`, `localhost`, and `169.254.169.254`.
  2. Running key-free discovery without internet access to paid APIs.
  3. Verifying output schema adherence.
