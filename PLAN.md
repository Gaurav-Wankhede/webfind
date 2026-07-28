# WebFind GUI Results & Categories Fix Plan

## Goal
Fix the GUI so that the main results window shows real search results (**Title + clickable URL + Description**) instead of raw SSE JSON, and replace the current "Top Domains" sidebar with **Google-style content-type filters**: All, Text, Images, News, Videos, etc.

---

## 1. Fix Main Results Window (Raw JSON → Title/URL/Description)

### Problem
The HTMX SSE container currently swaps `progress`, `result`, `error`, and `close` events into the same `<div>`, so the JSON progress payload is rendered as text.

### Fix
Use a custom JS handler (Option B) for explicit control over event routing.

### Changes

1. **`templates/search.html`**
   - Remove `sse-swap="message,progress,result,error,close"` from the SSE container.
   - Add explicit containers:
     ```html
     <div id="research-status">{% include "research_progress.html" %}</div>
     <div id="results-list" class="space-y-4"></div>
     <div id="error-message" class="text-red-400 hidden"></div>
     ```

2. **`assets/js/app.js`**
   - Add `initResearchStream()`:
     - Listen to `progress` events → update `#progress-text` + width of `#progress-bar`.
     - Listen to `result` events → set `#results-list.innerHTML = event.data`, hide `#research-status`.
     - Listen to `error` events → show `#error-message`.
     - Close the EventSource on `close`.

3. **`templates/research_progress.html`**
   - Keep as the initial spinner/progress indicator only.

---

## 2. Replace "Top Domains" Sidebar with Google-Style Content Filters

### Problem
The sidebar currently shows `Top Domains`, `Topic Tags`, `Content Types`. The user wants horizontal category chips like Google.

### Changes

1. **`templates/search.html`**
   - Replace the current `Categories` sidebar block with a horizontal row of filter chips:
     ```
     All | Text | Images | News | Videos | Documentation
     ```
   - The active chip should be highlighted.
   - Clicking a chip reloads `/web/search?q=...&type=...`.

2. **`src/gui/handlers.rs`**
   - Read `type` query parameter.
   - Pass `active_type` to the `SearchTemplate`.
   - Filter the crawled results by `content.content_type` before ranking.

3. **`src/gui/templates.rs`**
   - Add `active_type: String` to `SearchTemplate`.

---

## 3. Add Content-Type Classification to Backend

### Problem
`StructuredContent` does not carry a content-type label, so every `SearchResult` is `ContentType::Any` and the database cannot filter by type.

### Changes

1. **`src/schema/content.rs`**
   - Add fields:
     ```rust
     pub content_type: String,           // "text", "images", "videos", "news"
     pub content_type_header: String,    // original HTTP Content-Type header
     ```

2. **New file: `src/engine/content_classifier.rs`**
   - Implement `classify_content_type(content: &StructuredContent) -> &'static str`:
     - If `content_type_header` is `image/*` → `"images"`
     - If `content_type_header` is `video/*` → `"videos"`
     - If URL ends with image/video extensions → `"images"` / `"videos"`
     - If OpenGraph/Twitter type is `image`/`video` → map accordingly
     - If `schema_type` or content contains news markers → `"news"`
     - Default → `"text"`

3. **`src/engine/fetcher.rs`**
   - Store the response `Content-Type` header in `content_type_header`.
   - Call `classify_content_type()` and store the result in `content_type`.

4. **`src/engine/surreal_engine.rs`**
   - Persist `content_type` to `url_node.content_type`.
   - Update the `UPSERT` query to include `content_type = $content_type`.

---

## 4. Propagate Content Type Through Search Results

### Problem
`SearchResult.content_type` is hardcoded to `ContentType::Any` in multiple places.

### Changes

1. **`src/engine/search_engine.rs`**
   - Set `content_type` from the source `doc.content_type` when building `SearchResult`.

2. **`src/engine/surreal_engine.rs`**
   - Read `content_type` from the BM25 query row and include it in `SearchResult`.

3. **`src/engine/ranker.rs`**
   - Preserve `content_type` from input results instead of resetting to `Any`.

4. **`src/storage/tantivy_store.rs`**
   - Store `content_type` and return it in `to_search_result`.

5. **`src/engine/graph_summary.rs`**
   - Use the actual content type instead of `Any`.

---

## 5. Fix Result Card Layout

### Problem
The result card wraps the whole card in an indirect redirect link and does not display the URL clearly.

### Changes

1. **`templates/result_card.html`**
   - Render:
     - Favicon (if available)
     - Site name / domain
     - **Title** as a direct clickable link to `result.url`
     - **URL** shown as a small clickable line
     - **Description** from `result.snippet`
   - Example structure:
     ```html
     <article class="group mb-5">
       <div class="flex items-start gap-3">
         <img src="favicon" />
         <div>
           <div class="text-xs text-slate-400">site name / domain</div>
           <h3><a href="{{ result.url }}" target="_blank">{{ result.title }}</a></h3>
           <a href="{{ result.url }}" class="text-xs text-emerald-500 truncate">{{ result.url }}</a>
           <p class="text-sm text-slate-300 line-clamp-2">{{ result.snippet }}</p>
         </div>
       </div>
     </article>
     ```

2. **Optional:** Keep `/api/web/visit` only for analytics click tracking via a small `fetch()` call, not as the main navigation.

---

## 6. Fix Snippet Fallback

### Problem
Some results have empty `snippet` because the ranker defaults it to `""`.

### Changes

1. **`src/engine/ranker.rs`**
   - Do not overwrite `snippet` with `""`.
   - Fall back to `content.excerpt` or first 200 chars of `content_text`.

2. **`src/engine/graph_summary.rs`**
   - Same fallback behavior for generated results.

---

## 7. Files to Modify

| File | Change |
|---|---|
| `templates/search.html` | Separate SSE containers; add content-type chips |
| `templates/result_card.html` | New layout: title, URL, description |
| `templates/research_progress.html` | Keep as progress indicator |
| `assets/js/app.js` | Add custom SSE handler |
| `src/schema/content.rs` | Add `content_type` and `content_type_header` |
| `src/engine/fetcher.rs` | Store header; classify content |
| `src/engine/content_classifier.rs` | New classifier module |
| `src/engine/surreal_engine.rs` | Persist `content_type`; read it in search |
| `src/engine/search_engine.rs` | Carry `content_type` into result |
| `src/engine/ranker.rs` | Preserve `content_type`; fix snippet |
| `src/storage/tantivy_store.rs` | Store and return `content_type` |
| `src/engine/graph_summary.rs` | Use real `content_type`; fix snippet |
| `src/gui/handlers.rs` | Read `type` param; filter results |
| `src/gui/templates.rs` | Add `active_type` to template structs |
| `src/engine/mod.rs` | Wire up new `content_classifier` module |

---

## 8. Testing After Implementation

1. Run `cargo test` and fix any broken sample constructors.
2. Rebuild Docker image and verify:
   - `http://localhost:5750` loads styled.
   - Searching shows a progress bar, then actual result cards with title, URL, and description.
   - Clicking `Images` filter shows only image-type results.
   - No raw JSON appears in the results area.
3. Run `cargo test --lib` to verify library tests pass.
4. Run `cargo test` to verify all tests pass.

---

## 9. AI Agent Clean Content Extraction (Firecrawl-Inspired Filtering)

### Goal
Extract high-density, noise-free content for AI Agents by retaining only core semantic structural HTML elements (`<h1-h6>`, `<p>`, `<div>`, `<span>`), stripping away all web noise (scripts, styles, navigation bars, ad banners, and footers).

### Rationale
Raw HTML contains significant noise that wastes precious LLM context window tokens and degrades semantic search accuracy. Filtering down to structural headings, paragraphs, containers, and inline text mirrors Firecrawl's high-efficiency extraction strategy for AI agents.

### Plan & Implementation Tasks

1. **HTML Parser & Content Extractor (`src/engine/fetcher.rs` / DOM Extractor)**:
   - Filter DOM nodes during HTML parsing:
     - **Retain**: `<h1-h6>`, `<p>`, `<div>`, `<span>` (and standard inline formatting).
     - **Discard**: `<script>`, `<style>`, `<nav>`, `<header>`, `<footer>`, `<aside>`, `<form>`, `<button>`, `<head>`, `<noscript>`.
   - Normalize and collapse extra whitespace into clean, structured Markdown text.

2. **MCP Tool Responses (`src/mcp.rs`)**:
   - Ensure `webfind_search`, `webfind_research`, and `webfind_fetch` deliver this clean, high-density text format to AI Agents over MCP.

