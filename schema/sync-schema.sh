#!/bin/sh
# sync-schema.sh — Extract DDL from kavach-surreal and update WebFind/schema/
# Run from WebFind root: ./schema/sync-schema.sh
set -eu

KAVACH_SCHEMA="/Users/gauravwankhede/kavach-rs/crates/kavach-surreal/src/schema"
KAVACH_ENGINE="/Users/gauravwankhede/kavach-rs/crates/kavach-surreal/src"
OUT_DIR="$(dirname "$0")"

echo "Syncing schema from kavach-surreal..."

# Extract DDL from Rust raw strings
for f in unified core memory graph; do
  awk '/^pub\(super\) const DDL: &str = r"/{found=1; next} found && /^";/{exit} found' \
    "${KAVACH_SCHEMA}/${f}.rs" > "${OUT_DIR}/${f}.surql"
  echo "  ${f}.surql: $(wc -l < "${OUT_DIR}/${f}.surql") lines"
done

# schema_engine.surql is a standalone file
cp "${KAVACH_ENGINE}/schema_engine.surql" "${OUT_DIR}/schema_engine.surql"
echo "  schema_engine.surql: $(wc -l < "${OUT_DIR}/schema_engine.surql") lines"

# Combine into init.sql
cat "${OUT_DIR}/unified.surql" > "${OUT_DIR}/init.sql"
echo "" >> "${OUT_DIR}/init.sql"
cat "${OUT_DIR}/schema_engine.surql" >> "${OUT_DIR}/init.sql"
echo "  init.sql: $(wc -l < "${OUT_DIR}/init.sql") lines (combined)"

echo "Schema sync complete."
