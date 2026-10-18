# Embedded Mode Configurations

## Overview
Configuration for running WebFind with all databases embedded locally (no external services).

---

## Turso (libSQL) Embedded

### Database File
```rust
let turso_url = "file:///var/lib/webfind/webfind.db";
// Or in-memory for testing: "file::memory:?cache=shared"
```

### Features Enabled
```toml
libsql = { version = "0.6", features = ["core", "encryption", "replication"] }
```

### DiskANN Index Configuration
```sql
-- Vector index for approximate nearest neighbor search
CREATE INDEX documents_embedding_idx 
ON documents (libsql_vector_idx(embedding, 'type=diskann', 'metric=cosine'));
```

### FTS5 Configuration
```sql
-- Porter stemmer with Unicode support
CREATE VIRTUAL TABLE documents_fts USING fts5(
    title,
    content_text,
    content_markdown,
    url,
    domain,
    tokenize = 'porter unicode61'
);
```

### Encryption (Optional)
```rust
let encryption_key = Some("base64-encoded-32-byte-key");
let store = TursoStore::new_with_encryption(path, encryption_key).await?;
```

---

## Qdrant Edge (Embedded)

### Binary Deployment
```bash
# Download Qdrant Edge
wget https://github.com/qdrant/qdrant/releases/download/v1.9.0/qdrant-x86_64-unknown-linux-gnu.tar.gz
tar -xzf qdrant-*.tar.gz
./qdrant --config-path ./qdrant-config.yaml
```

### Configuration (qdrant-config.yaml)
```yaml
storage:
  storage_path: ./qdrant_data
  snapshots_path: ./qdrant_snapshots

service:
  http_port: 6334
  grpc_port: 6335
  enable_cors: true

log_level: INFO

# Disable telemetry
telemetry_disabled: true

# Limits for embedded mode
limits:
  max_request_size_mb: 32
  max_search_results: 1000
```

### Collection Configuration (Auto-created)
```rust
let collection = CreateCollection {
    collection_name: "webfind_embeddings".to_string(),
    vectors_config: Some(VectorsConfig {
        config: Some(vectors_config::Config::Params(VectorParams {
            size: 768,                    // EmbeddingGemma2 default
            distance: Distance::Cosine.into(),
            hnsw_config: Some(HnswConfigDiff {
                m: Some(16),
                ef_construct: Some(100),
                full_scan_threshold: Some(10000),
                on_disk: Some(true),      // Disk-backed for embedded
            }),
            quantization_config: Some(QuantizationConfig {
                scalar: Some(ScalarQuantization {
                    r#type: ScalarQuantizationType::Int8.into(),
                    quantile: Some(0.99),
                    always_ram: Some(true),
                }),
            }),
            on_disk: Some(true),
            ..Default::default()
        })),
    }),
    sparse_vectors_config: Some(SparseVectorsConfig {
        map: vec![(
            "bm25".to_string(),
            SparseVectorParams {
                index: Some(SparseIndexConfig {
                    on_disk: Some(false),  // Keep sparse in memory
                    full_scan_threshold: Some(10000),
                }),
            },
        )].into_iter().collect(),
    }),
    ..Default::default()
};
```

---

## FalkorDBLite (Embedded)

### Deployment
```bash
# FalkorDB Lite runs as embedded in-process via Redis protocol
# No separate binary needed - uses redis crate directly

# For standalone: docker run -p 6379:6379 falkordb/falkordb:latest
```

### Graph Schema Initialization
```cypher
// Create indexes for performance
CREATE INDEX ON :Document(url);
CREATE INDEX ON :Entity(name);
CREATE INDEX ON :Concept(name);

// Fulltext search on entities
CALL db.idx.fulltext.createNodeIndex("entityNames", ["Entity"], ["name"]);

// Vector index for entity embeddings (if using vector similarity)
CALL db.idx.vector.createNodeIndex("entityEmbeddings", "Entity", "embedding", 
    {dimension: 768, similarity: "cosine"});
```

### Embedded Redis Connection
```rust
use redis::Client;

let client = Client::open("redis://localhost:6379")?;
// Or for in-process: use redis crate with embedded server feature
```

---

## Single-File Deployment Structure

```
webfind/
├── webfind              # Main binary
├── webfind.db           # Turso database file
├── qdrant_data/         # Qdrant storage
├── qdrant_snapshots/    # Qdrant snapshots
├── qdrant-config.yaml   # Qdrant config
├── config.toml          # WebFind config
└── logs/                # Log files
```

---

## Configuration File (config.toml)

```toml
[database]
turso_path = "file:///var/lib/webfind/webfind.db"
turso_encryption_key = ""  # Base64, optional

[qdrant]
url = "http://localhost:6334"
collection_name = "webfind_embeddings"

[falkordb]
url = "redis://localhost:6379"
graph_name = "webfind_kag"

[pipeline]
max_concurrent_fetches = 10
fetch_timeout_secs = 30
enable_kag = true
enable_explanations = true
max_examples_per_type = 5

[models]
embedding_model = "embeddinggemma2-270m"
embedding_dim = 768
explanation_model = "qwen2.5-0.5b-instruct"
explanation_model_path = "/var/lib/webfind/models/qwen2.5-0.5b-instruct-q4_k_m.gguf"

[search]
default_depth = "standard"
max_sources = 20
max_results = 50
include_images = true
default_languages = ["en"]

[cache]
query_ttl_hours = 24
embedding_ttl_days = 30
fetch_ttl_hours = 168
```

---

## Systemd Service (Linux)

```ini
[Unit]
Description=WebFind Search Engine
After=network.target

[Service]
Type=simple
User=webfind
WorkingDirectory=/opt/webfind
ExecStart=/opt/webfind/webfind serve
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info

# Resource limits for embedded mode
MemoryLimit=2G
CPUQuota=200%

[Install]
WantedBy=multi-user.target
```

---

## Docker Compose (Development)

```yaml
version: '3.8'
services:
  qdrant:
    image: qdrant/qdrant:v1.9.0
    ports:
      - "6334:6334"
      - "6335:6335"
    volumes:
      - ./qdrant_data:/qdrant/storage
      - ./qdrant_config.yaml:/qdrant/config/production.yaml
  
  falkordb:
    image: falkordb/falkordb:latest
    ports:
      - "6379:6379"
    volumes:
      - ./falkordb_data:/data
  
  webfind:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - ./webfind.db:/var/lib/webfind/webfind.db
      - ./config.toml:/etc/webfind/config.toml
    depends_on:
      - qdrant
      - falkordb
    environment:
      - RUST_LOG=info
      - TURSO_URL=file:///var/lib/webfind/webfind.db
      - QDRANT_URL=http://qdrant:6334
      - FALKOR_URL=redis://falkordb:6379
```

---

*End of Embedded Mode Configuration Specification*
