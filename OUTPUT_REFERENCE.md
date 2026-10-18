# WebFind — Output Reference (production pipeline)

*Generated after the pipeline was wired to real implementations. All values below
are captured from actual runs.*

## 1. `webfind fetch <URL>`

### Report format

```text
┌─────────────────────────────────────────────────────────────┐
│                    WEBFIND FETCH REPORT                    │
├─────────────────────────────────────────────────────────────┤
│ Status:       OK                                           │
├─────────────────────────────────────────────────────────────┤
│ URL:          https://en.wikipedia.org/wiki/SMILES         │
│ Title:        Simplified Molecular Input Line Entry System │
│ Language:     eng (100%)                                   │
│ Words:        1905                                         │
│ Reading time: 481s                                         │
│ Reading ease: 13.8                                         │
│ Grade level:  17.2                                         │
│ SSL:          valid                                        │
│ Paywalled:    no                                           │
│ Fetched in:   265ms                                        │
├─────────────────────────────────────────────────────────────┤
│ EXCERPT                                                   │
├─────────────────────────────────────────────────────────────┤
│ Atoms are represented by the standard abbreviation of the   │
│ chemical elements , in square brackets, such as [Au] for    │
│ gold . Brackets may be omitted in the common case of atoms  │
│ which:                                                      │
├─────────────────────────────────────────────────────────────┤
│ KEYWORDS                                                  │
├─────────────────────────────────────────────────────────────┤
│ article (66.0), asturianu (66.0), azərbaycanca (66.0),      │
│ bahasa (66.0), bokmål (66.0), bosanski (66.0), català       │
│ (66.0), dansk (66.0), deutsch (66.0), english (66.0)        │
└─────────────────────────────────────────────────────────────┘

```

### JSON format

```json
{
  "status": "OK",
  "url": "https://en.wikipedia.org/wiki/SMILES",
  "title": "Simplified Molecular Input Line Entry System",
  "language": "eng",
  "language_confidence": 1.00,
  "word_count": 1905,
  "reading_time_seconds": 481,
  "reading_ease": 13.8,
  "grade_level": 17.2,
  "status_code": 200,
  "ssl_valid": true,
  "is_paywalled": false,
  "is_valid_content": true,
  "excerpt": "Atoms are represented by the standard abbreviation of the chemical elements , in square brackets, such as [Au] for gold . Brackets may be omitted in the common case of atoms which:",
  "content_markdown": "<30539 chars>",
  "keywords": ["article", "asturianu", "az\u0259rbaycanca", "bahasa", "bokm\u00e5l", "bosanski", "catal\u00e0", "dansk"],
  "entities": {
    "code_blocks": 11,
    "tables": 12,
    "headings": 20,
    "diagrams": 0,
    "examples": 0,
    "maths": 0
  }
}
```

## 2. Match-based example extraction

The pipeline emits a typed example **only** when the page actually contains a
detectable artifact. Pages with none produce an empty list — that is the gate.

| Artifact | Detection | `ExampleType` |
|---|---|---|
| Fenced code ```` ```rust ```` | regex + DOM `<pre>`/`<code>` | `Code` |
| LaTeX `$$...$$`, `\[...\]` | regex | `Math` |
| SMILES / InChI | chemistry-shape heuristic | `Chemical` |
| Markdown table | header + delimiter row | `Table` |
| Mermaid / dot / plantuml fence | regex | `Diagram` |
| Markdown image `![alt](url)` | regex | `Image` |

Verified on live pages:

```
https://en.wikipedia.org/wiki/SMILES              code=11 tables=12 headings=20
https://doc.rust-lang.org/std/ops/trait.Fn.html   code=4  tables=0  headings=18
https://doc.rust-lang.org/book/ch16-00-concurrency.md code=0 tables=0  headings=3
```

## 3. Pipeline stages (all real, no servers)

| Stage | Implementation | Storage |
|---|---|---|
| Route | `engine::query_router` — 4-layer cascade | in-process |
| Embed | `engine::embedder::FastembedEmbedder` (ONNX) | in-process, cached in Turso |
| Retrieve | `storage::turso_store::TursoStore::search` — BM25 + DiskANN + PageRank | embedded Turso |
| Fetch | `engine::fetcher::Fetcher` | HTTP |
| Extract | `pipeline::example_extract::extract_examples` | match-based |
| Synthesize | `pipeline::main::Pipeline` | in-process |

**No Qdrant, no FalkorDB, no Redis, no external servers.** The embedded
Turso/libSQL store already provides FTS5 lexical search, DiskANN vector search,
the crawl link graph, PageRank, and cache tables.

## 4. Defects fixed

| Defect | Root cause | Fix |
|---|---|---|
| Blank EXCERPT on every page | excerpt only came from readability/meta; no body fallback | `derive_excerpt()` falls back to extracted text |
| Blank `content_markdown` on SPAs | Angular/React shell (`<app-root>`) yields no HTML | `is_empty_shell()` + `text_to_html()` fallback |
| JS source in keywords | readability returned `<script>` bodies | `strip_code_blocks()` + `looks_like_javascript()` |
| CSS in keywords | `<style>` body emitted as text | `looks_like_css()` + `strip_junk_segments()` |
| Zero code blocks on docs.rs | only `<pre><code>` scanned | bare `<code>` extraction added |
| `class="src"` stripped | rustdoc marks code blocks `class="src"`; treated as noise | narrowed to `src-link`/`srclink` |
| Absurd reading scores (`-630`) | JS/CSS counted as prose | code/CSS stripped before scoring |
| `fetch_and_extract` returned `Vec::new()` | unimplemented stub | real concurrent fetch + extractor |
| Pipeline never invoked | `search.rs` bypassed it entirely | `extract_examples_for_hits()` wired in |

## 5. Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | general error |
| 101 | panic (should not occur) |

---

*WebFind v0.5.0 — embedded pipeline, no external services required.*
