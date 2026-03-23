#!/bin/sh
# Download a backup from S3 and restore it into PostgreSQL.
#
# Usage:
#   /restore.sh                    # restore the latest daily backup
#   /restore.sh daily/yoink-20260323T030000Z.sql.gz   # restore a specific file
#   /restore.sh list               # list available backups
#
# Required env: same as backup.sh

set -eu

: "${PGHOST:?PGHOST is required}"
: "${PGUSER:?PGUSER is required}"
: "${PGPASSWORD:?PGPASSWORD is required}"
: "${PGDATABASE:?PGDATABASE is required}"
: "${BACKUP_S3_ENDPOINT:?BACKUP_S3_ENDPOINT is required}"
: "${BACKUP_S3_BUCKET:?BACKUP_S3_BUCKET is required}"
: "${BACKUP_S3_ACCESS_KEY:?BACKUP_S3_ACCESS_KEY is required}"
: "${BACKUP_S3_SECRET_KEY:?BACKUP_S3_SECRET_KEY is required}"

mc alias set s3bak \
  "${BACKUP_S3_ENDPOINT}" \
  "${BACKUP_S3_ACCESS_KEY}" \
  "${BACKUP_S3_SECRET_KEY}" \
  --api S3v4 \
  --path auto \
  >/dev/null 2>&1

log() { echo "[restore] $(date -u +%H:%M:%S) $*"; }

TARGET="${1:-latest}"

if [ "$TARGET" = "list" ]; then
  echo "Daily backups:"
  mc ls "s3bak/${BACKUP_S3_BUCKET}/daily/" 2>/dev/null || echo "  (none)"
  echo ""
  echo "Weekly backups:"
  mc ls "s3bak/${BACKUP_S3_BUCKET}/weekly/" 2>/dev/null || echo "  (none)"
  exit 0
fi

if [ "$TARGET" = "latest" ]; then
  TARGET=$(mc ls "s3bak/${BACKUP_S3_BUCKET}/daily/" --json 2>/dev/null \
    | grep '"key"' \
    | sed 's/.*"key":"\([^"]*\)".*/\1/' \
    | sort -r \
    | head -1)
  if [ -z "$TARGET" ]; then
    log "No backups found in daily/"
    exit 1
  fi
  TARGET="daily/${TARGET}"
  log "Latest backup: ${TARGET}"
fi

TMPFILE=$(mktemp -t restore-XXXXXX)
trap 'rm -f "$TMPFILE"' EXIT

log "Downloading ${TARGET}..."
mc cp "s3bak/${BACKUP_S3_BUCKET}/${TARGET}" "$TMPFILE" >/dev/null
SIZE=$(wc -c < "$TMPFILE" | tr -d ' ')
log "Downloaded: ${SIZE} bytes"

log "Restoring into ${PGDATABASE}@${PGHOST}..."
log "WARNING: This will drop and recreate all objects in the database."

pg_restore \
  --no-owner \
  --no-privileges \
  --clean \
  --if-exists \
  -d "$PGDATABASE" \
  "$TMPFILE" \
  2>&1 || true

log "Restore complete."
