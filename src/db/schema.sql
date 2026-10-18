-- WebFind Database Schema
-- Run this to initialize the Turso/libSQL database

-- ==================== DOCUMENTS ====================
CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    content_markdown TEXT NOT NULL,
    content_text TEXT NOT NULL,
    domain TEXT NOT NULL,
    language TEXT NOT NULL DEFAULT 'en',
    source_tier TEXT NOT NULL DEFAULT 'unknown',
    fetched_at DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    embedding BLOB  -- F32_BLOB(768) for exact vector search
);

-- FTS5 for BM25 full-text search
CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
    title,
    content_text,
    content_markdown,
    url,
    domain,
    tokenize = 'porter unicode61'
);

-- Triggers to keep FTS5 in sync
CREATE TRIGGER IF NOT EXISTS documents_fts_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts (title, content_text, content_markdown, url, domain)
    VALUES (new.title, new.content_text, new.content_markdown, new.url, new.domain);
END;

CREATE TRIGGER IF NOT EXISTS documents_fts_delete AFTER DELETE ON documents BEGIN
    DELETE FROM documents_fts WHERE url = old.url;
END;

CREATE TRIGGER IF NOT EXISTS documents_fts_update AFTER UPDATE ON documents BEGIN
    DELETE FROM documents_fts WHERE url = old.url;
    INSERT INTO documents_fts (title, content_text, content_markdown, url, domain)
    VALUES (new.title, new.content_text, new.content_markdown, new.url, new.domain);
END;

-- DiskANN index for approximate nearest neighbor search
CREATE INDEX IF NOT EXISTS documents_embedding_idx 
ON documents (libsql_vector_idx(embedding, 'type=diskann', 'metric=cosine'));

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_documents_domain ON documents(domain);
CREATE INDEX IF NOT EXISTS idx_documents_source_tier ON documents(source_tier);
CREATE INDEX IF NOT EXISTS idx_documents_fetched_at ON documents(fetched_at DESC);

-- ==================== CRAWL GRAPH ====================
CREATE TABLE IF NOT EXISTS crawl_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL DEFAULT '',
    domain TEXT NOT NULL,
    source_tier TEXT NOT NULL DEFAULT 'unknown',
    pagerank REAL NOT NULL DEFAULT 1.0,
    fetched_at DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS crawl_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_url TEXT NOT NULL,
    target_url TEXT NOT NULL,
    anchor_text TEXT,
    weight REAL NOT NULL DEFAULT 1.0,
    edge_type TEXT NOT NULL DEFAULT 'content',
    discovered_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (source_url, target_url)
);

CREATE INDEX IF NOT EXISTS idx_crawl_edges_source ON crawl_edges(source_url);
CREATE INDEX IF NOT EXISTS idx_crawl_edges_target ON crawl_edges(target_url);
CREATE INDEX IF NOT EXISTS idx_crawl_nodes_domain ON crawl_nodes(domain);
CREATE INDEX IF NOT EXISTS idx_crawl_nodes_pagerank ON crawl_nodes(pagerank DESC);

-- ==================== CACHES ====================
CREATE TABLE IF NOT EXISTS query_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    query_hash TEXT NOT NULL UNIQUE,
    query_text TEXT NOT NULL,
    results_json TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS embedding_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    text_hash TEXT NOT NULL UNIQUE,
    text_preview TEXT NOT NULL,
    embedding BLOB NOT NULL,  -- F32_BLOB(768)
    model_name TEXT NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS fetch_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    url_hash TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL,
    response_json TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    content_hash TEXT,
    fetched_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at DATETIME NOT NULL
);

-- ==================== SESSION & AUDIT ====================
CREATE TABLE IF NOT EXISTS session_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL UNIQUE,
    value_json TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    event_type TEXT NOT NULL,
    details_json TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'info'
);

CREATE INDEX IF NOT EXISTS idx_query_cache_expires ON query_cache(expires_at);
CREATE INDEX IF NOT EXISTS idx_embedding_cache_text_hash ON embedding_cache(text_hash);
CREATE INDEX IF NOT EXISTS idx_fetch_cache_expires ON fetch_cache(expires_at);
CREATE INDEX IF NOT EXISTS idx_session_state_key ON session_state(session_key);
CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_severity ON audit_log(severity);