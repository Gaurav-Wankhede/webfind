# Migrating to WebFind 2.0 (Single-Binary Turso)

> **From:** WebFind with a separate SurrealDB container (`docker compose` with `surrealdb` + `schema-init`).
> **To:** WebFind 2.0 — a single container with an embedded Turso/libSQL database. No SurrealDB, no separate database, no schema-init step.

WebFind 2.0 embeds its storage (link graph, FTS, DiskANN vector, PageRank) in a single SQLite-compatible Turso file. The database schema is created automatically on first run — there is no `schema/init.sql` to apply and no external database service.

---

## Step 1 — Back up your SurrealDB data (if migrating a live index)

The link graph is the only durable state worth migrating (page bodies and crawl jobs are re-derivable). Export it to JSON:

```bash
# While SurrealDB is still running, export the graph state:
surrealdb export --conn ws://localhost:7710 --user root --pass root --ns kavach --db main > webfind_export.json
```

> **Note:** The export must be in the WebFind JSON export format (`{"version":1,"url_nodes":[...],"link_edges":[...]}`). If you have a raw SurrealDB dump, convert it to this shape — `url_nodes` need `url` (+ optional `excerpt`, `embedding`), `link_edges` need `from`/`to`.

## Step 2 — Update `docker-compose.yml`

**Before (SurrealDB + schema-init + webfind):** a 3-service file with `surrealdb`, `schema-init`, and `webfind` pointing at `ws://surrealdb:7710`.

**After (single service):** delete the `surrealdb` and `schema-init` services. The `webfind` service needs only:

```yaml
services:
  webfind:
    build: .
    container_name: webfind-server
    ports:
      - "5748:4747"   # API / MCP (Streamable HTTP)
      - "5750:4749"   # HTML GUI
    environment:
      WEBFIND_DATA_DIR: /data
      WEBFIND_GRAPH_STORE: turso
      WEBFIND_TURSO_PATH: /data/webfind.db
      WEBFIND_RATE_LIMIT: "60"
    volumes:
      - webfind-data:/data
    command: ["serve", "--transport", "http", "--gui-port", "4749"]
    restart: unless-stopped

volumes:
  webfind-data:
```

Remove all `WEBFIND_SURREAL_*` env vars and the `schema/` volume mount.

## Step 3 — Bring it up

```bash
docker compose up -d --build
```

The Turso database is created at `/data/webfind.db` on first start. The Web UI is at `http://localhost:5750`, the API/MCP at `http://localhost:5748`.

## Step 4 — Migrate your graph (optional)

If you exported your graph in Step 1, import it into the new Turso database:

```bash
# Copy the export into the container and run the migration tool:
docker cp webfind_export.json webfind-server:/tmp/export.json
docker exec webfind-server webfind migrate --from /tmp/export.json --to /data/webfind.db
```

This re-derives BLAKE3 URL IDs, validates embedding dimensions, imports the graph, rebuilds the FTS index, and recomputes PageRank.

## Step 5 — Verify

```bash
docker exec webfind-server webfind status
# Should show the embedded Turso store active and index healthy.
```

---

## Backup & restore

WebFind's backup story is a **file copy** — no `surrealdb export`:

```bash
# Backup (while the container is stopped, to get a consistent snapshot):
docker compose stop webfind
docker cp webfind-server:/data/webfind.db ./backup.db
docker compose start webfind

# Restore:
docker cp ./backup.db webfind-server:/data/webfind.db
```

## Reverting

WebFind 2.0 has no SurrealDB backend to revert to — the SurrealDB backend was removed. If you need the old architecture, stay on the pre-migration commit/tag. The migration itself is non-destructive to your SurrealDB export (it reads it; it never deletes).
