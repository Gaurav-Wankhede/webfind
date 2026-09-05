# PRD — Curated Seed Catalog & Background Curation Daemon

> **Feature:** A modular, non-Wikipedia curated seed catalog feeding a background daemon that continuously crawls and indexes authoritative sources into the local, size-bounded Turso index.
> **Status:** Core implementation DONE (see [Implementation Status](#implementation-status)). Remaining roadmap items listed for future work.
> **Date:** 2026-08-12
> **Design principle:** *Never seed Wikipedia. Prefer official docs, RFCs, standards bodies, primary registries, and vendor sources so extracted content is authoritative and the crawled HTML is dense, high-signal, and LLM-friendly.*

---

## 1. Problem Statement

WebFind starts every session with an **empty index**. All retrieval is "crawl-as-you-go" — the index only grows when a user actively runs `research` or `crawl`. This means:

- **Cold-start latency:** the first query has nothing to search against.
- **No topic awareness:** the system has no prior understanding of which domains are authoritative for a given subject.
- **No freshness:** content is only as current as the last manual crawl.
- **Unbounded storage:** full page content accumulates with no retention policy, which is fatal on machines with limited disk.

### Customers / Users Affected

- Self-hosters running WebFind on laptops, VPS, or Raspberry Pi (limited storage).
- Users of free / local LLMs (OpenCode, Claude Code, LM Studio) who need offline, private web research.
- Anyone who wants deterministic, topic-focused, always-fresh local search without a paid retrieval API (Exa, Parallel).

---

## 2. Goals / Non-Goals

### Goals

1. **Pre-populated topic awareness** — the local index knows authoritative, official source URLs for 24+ domains (research, programming, medical, finance, banking, tech, security, law, science, energy, AI/ML, etc.) before any user query.
2. **Continuous freshness** — a background daemon re-crawls sources on per-source cadences (daily / weekly / monthly) so the index stays current without manual action.
3. **Size-bounded storage** — the index stays under a configurable disk budget by tiered retention (keep embedding + excerpt + entities; evict full text of the oldest pages).
4. **Modular curation** — each domain lives in its own `.rs` file so sources can be expanded independently and holistically.
5. **No Wikipedia** — every seed is an official/primary/standards source.

### Non-Goals

- Matching Exa's *scale* (1B people, 50M companies, 350M publications). We cannot and will not download curated mega-databases; we build a **focused, fresh, size-bounded** local index instead.
- Cloud / hosted operation — this is a local, single-binary feature.
- Commercial licensing of the curated list — sources are public URLs.

---

## 3. Success Metrics

| Metric | Target |
|---|---|
| Curated domains | ≥ 24 |
| Curated source URLs | ≥ 150, all non-Wikipedia, all HTTPS |
| Cold-start search coverage | Query against a curated domain returns results **without** a prior manual crawl |
| Storage bound | DB stays under `max_bytes` budget while daemon runs indefinitely |
| Freshness | High-churn domains re-crawled ≥ 1×/day; stable docs ≥ 1×/month |
| Robustness | Catalog tests enforce: no-Wikipedia, all-HTTPS, unique slugs, non-empty |

---

## 4. Solution Overview

Three layers, each independently built and already committed:

```
┌─────────────────────────────────────────────────────────────┐
│  1. Seed Catalog (modular)                                   │
│     src/engine/seed_catalog/*.rs  — one file per domain      │
│     24 domains, 171 official (non-Wikipedia) source URLs     │
├─────────────────────────────────────────────────────────────┤
│  2. Curation Daemon                                          │
│     src/engine/curation_daemon.rs                            │
│     Sweeps catalog, crawls each source on recrawl cadence,   │
│     persists content + embeddings via BackgroundWorker       │
├─────────────────────────────────────────────────────────────┤
│  3. Storage Budget (size-bounded retention)                  │
│     src/storage/turso_store.rs + src/config.rs               │
│     Evicts full text of oldest pages; keeps searchable core  │
└─────────────────────────────────────────────────────────────┘
```

### 4.1 Seed Catalog (DONE)

- `src/engine/seed_catalog/mod.rs` — shared types (`SeedSource`, `Recrawl`, `CuratedDomain`), the `DOMAINS` aggregate, lookup/helpers, and contract tests.
- One `*.rs` file per domain (`research.rs`, `programming.rs`, `medical.rs`, `finance.rs`, `banking.rs`, `tech.rs`, `cybersecurity.rs`, `legal.rs`, `science.rs`, `energy.rs`, `ai_ml.rs`, `data_science.rs`, `marketing.rs`, `business.rs`, `ecommerce.rs`, `education.rs`, `government.rs`, `gaming.rs`, `automotive.rs`, `sports.rs`, `travel.rs`, `food.rs`, `open_source.rs`, `crypto.rs`).
- Each file defines `pub const DOMAIN: CuratedDomain` (slug, label, topics, sources).

**How to add a domain (future curation):**
1. Create `src/engine/seed_catalog/<slug>.rs` with `pub const DOMAIN: CuratedDomain`.
2. Register in `mod.rs`: add `pub mod <slug>;` and append `<slug>::DOMAIN` to the `DOMAINS` array.

**How to expand a domain:** add `SeedSource { url: ..., recrawl: ... }` entries to that domain's `sources` array.

### 4.2 Curation Daemon (DONE)

- `src/engine/curation_daemon.rs` — `CurationDaemon` + `DaemonConfig`.
- `run_forever()`: loops, calling `sweep()` each interval.
- `sweep()`: for each due source, crawl via `BulkDomainCrawler` (topic-aware prioritization, robots.txt respected, external links ignored, rate-limited), persist via `BackgroundWorker`.
- Per-source recrawl cadence via `Recrawl` enum (`Daily` / `Weekly` / `Monthly`).
- Config: `delay_ms`, `max_pages_per_source`, `sweep_interval_secs`, `only_domains`.

**CLI:**
```bash
webfind index domains                          # list the curated catalog
webfind crawl --daemon                          # run all domains forever
webfind crawl --daemon --domains finance,tech   # subset only
webfind crawl --daemon --daemon-pages 100 --daemon-interval 1800
```

### 4.3 Storage Budget (DONE)

- `src/config.rs` — `StorageConfig { max_bytes, max_full_content_pages }` + resolvers + `parse_bytes("500MB")`.
- `src/storage/turso_store.rs` — `db_size_bytes()`, `full_content_page_count()`, `enforce_storage_budget()`, `evict_full_content()`.
- `src/commands/serve.rs` — periodic (60s) budget enforcer for both transports.
- Eviction strips full text/markdown/html of **oldest** pages, preserving embeddings + excerpts (searchable core).

**Config:**
```toml
# webfind.toml
[storage]
max_bytes = "500MB"            # or WEBFIND_STORAGE_MAX_BYTES=500MB
max_full_content_pages = 5000  # or WEBFIND_STORAGE_MAX_FULL_CONTENT=5000
```

---

## 5. User Stories

1. **As a** self-hoster on a 500MB VPS, **I want** the daemon to run in the background, **so that** my index grows and stays fresh without ever filling the disk.
2. **As a** researcher using a free local LLM, **I want** authoritative sources for finance and research pre-seeded, **so that** my first query returns high-quality, non-Wikipedia results.
3. **As a** developer, **I want** to curate the seed catalog per domain, **so that** I can expand coverage holistically without touching unrelated code.
4. **As a** maintainer, **I want** automated catalog contract tests, **so that** no domain can be added with Wikipedia sources, insecure URLs, or duplicate slugs.

---

## 6. Acceptance Criteria

1. `webfind index domains` lists ≥ 24 domains and ≥ 150 total sources.
2. No source URL contains `wikipedia.org`; all are `https://`.
3. All slugs are unique; every domain has ≥ 1 source (enforced by tests).
4. `webfind crawl --daemon` runs continuously, re-crawling on cadence, and the DB stays under the configured `max_bytes` budget.
5. `webfind crawl --daemon --domains <slug>` crawls only the requested subset.
6. Storage eviction preserves embeddings + excerpts; only full text is stripped for the oldest pages.
7. **120/120 lib tests pass**, including the 5 catalog contract tests and 2 storage-budget tests.

---

## 7. Edge Cases

- **Source gone / DNS fail:** daemon logs a warning, counts an error, and continues to the next source.
- **Empty index cold start:** daemon populates it; if a source yields no valid content it is skipped, not fatal.
- **Storage budget exhausted with nothing evictable:** `enforce_storage_budget` returns 0 gracefully (no infinite loop); DB simply exceeds budget rather than erroring.
- **In-memory (`:memory:`) store:** `db_size_bytes()` falls back to a logical-size approximation so eviction still has a signal in tests.
- **Unknown `--domains` slug:** silently produces an empty active-domain set (daemon crawls nothing) — consider warning.

---

## 8. Open Questions / Future Roadmap

| # | Item | Status | Notes |
|---|------|--------|-------|
| 1 | **RSS / sitemap ingestion** | Future | Subscribe to official RSS feeds per domain for the freshest article URLs without blind crawling. Biggest lever for a "publications-like" index. |
| 2 | **JS-rendered sources** | Future | Enable headless Chromium (`--dynamic`) for JS-heavy pages (some vendor blogs). |
| 3 | **Unknown-slug warning** | Future | `--domains foo` with no match should print a warning listing valid slugs. |
| 4 | **Custom user catalog** | Future | Allow a user-supplied TOML/JSON catalog that merges with (or overrides) the built-in one. |
| 5 | **Relevance-based eviction** | Future | Evict by score (PageRank/relevance) rather than purely `fetched_at`, so low-value pages go first. |
| 6 | **Daemon health/status endpoint** | Future | `webfind status` or API endpoint reporting last-sweep stats, per-source last-crawl time, DB size / budget. |
| 7 | **Persisted recrawl state** | Future | Currently recrawl timing is in-memory (`Instant` per process); persist last-crawl time to Turso so restarts honor cadence. |
| 8 | **Embedding on daemon** | Future | Daemon currently passes `None` embedder; wire `FastembedEmbedder` so vectors are generated for hybrid search. |
| 9 | **More domains** | Future | Expand beyond 24 (e.g., agriculture, logistics, HR, real estate, telecom, manufacturing). Add as `seed_catalog/<slug>.rs`. |

---

## 9. Implementation Status

| Component | Status | Files |
|---|---|---|
| Seed catalog (24 domains / 171 sources) | ✅ DONE | `src/engine/seed_catalog/` |
| Catalog contract tests (5) | ✅ DONE | `src/engine/seed_catalog/mod.rs` |
| Curation daemon | ✅ DONE | `src/engine/curation_daemon.rs` |
| Daemon CLI (`--daemon`, `--domains`, etc.) | ✅ DONE | `src/cli.rs`, `src/commands/crawl.rs`, `src/main.rs` |
| `index domains` command | ✅ DONE | `src/commands/index.rs` |
| Storage budget + tiered eviction | ✅ DONE | `src/config.rs`, `src/storage/turso_store.rs`, `src/commands/serve.rs` |
| Storage budget tests (2) | ✅ DONE | `src/storage/turso_store.rs` |
| **RSS ingestion** | 🔲 Future | Roadmap #1 |
| **JS-rendered source support** | 🔲 Future | Roadmap #2 |
| **Persisted recrawl state** | 🔲 Future | Roadmap #7 |
| **Embedding in daemon** | 🔲 Future | Roadmap #8 |

**Test status:** `cargo test --release --lib` → **120 passed, 0 failed** (115 pre-existing + 5 catalog + 2 storage, minus overlaps). Build clean.

---

## 10. Risks & Mitigations

- **Source block / bot detection:** daemon uses `BulkDomainCrawler` with session consistency, UA rotation, rate limiting, and robots.txt compliance. Mitigation: conservative `delay_ms`, per-domain pacing.
- **Catalog staleness:** curated URLs may drift. Mitigation: modular files make updates cheap; recrawl cadence surfaces failures in logs.
- **Storage still grows despite budget:** eviction only strips full text; embeddings+excerpts still consume space. Mitigation: set `max_bytes` conservatively; roadmap #5 (relevance eviction) reduces low-value stubs.
- **Scope creep (matching Exa scale):** explicitly a non-goal; the value is focused freshness + bounded storage, not breadth.
