#!/bin/sh
# pg_dump -> gzip -> upload to S3 with retention policy.
# Runs as a one-shot or on a cron schedule inside the backup container.
#
# Required env:
#   PGHOST, PGUSER, PGPASSWORD, PGDATABASE
#   BACKUP_S3_ENDPOINT, BACKUP_S3_BUCKET
#   BACKUP_S3_ACCESS_KEY, BACKUP_S3_SECRET_KEY
#
# Optional env:
#   BACKUP_S3_REGION       (default: garage)
#   BACKUP_RETAIN_DAILY    (default: 7)
#   BACKUP_RETAIN_WEEKLY   (default: 4)

set -eu

: "${PGHOST:?PGHOST is required}"
: "${PGUSER:?PGUSER is required}"
: "${PGPASSWORD:?PGPASSWORD is required}"
: "${PGDATABASE:?PGDATABASE is required}"
: "${BACKUP_S3_ENDPOINT:?BACKUP_S3_ENDPOINT is required}"
: "${BACKUP_S3_BUCKET:?BACKUP_S3_BUCKET is required}"
: "${BACKUP_S3_ACCESS_KEY:?BACKUP_S3_ACCESS_KEY is required}"
: "${BACKUP_S3_SECRET_KEY:?BACKUP_S3_SECRET_KEY is required}"

S3_REGION="${BACKUP_S3_REGION:-garage}"
RETAIN_DAILY="${BACKUP_RETAIN_DAILY:-7}"
RETAIN_WEEKLY="${BACKUP_RETAIN_WEEKLY:-4}"

TIMESTAMP=$(date -u +%Y%m%dT%H%M%SZ)
DAY_OF_WEEK=$(date -u +%u)
FILENAME="${PGDATABASE}-${TIMESTAMP}.dump"

# Configure mc (MinIO client) alias
mc alias set s3bak \
  "${BACKUP_S3_ENDPOINT}" \
  "${BACKUP_S3_ACCESS_KEY}" \
  "${BACKUP_S3_SECRET_KEY}" \
  --api S3v4 \
  --path auto \
  >/dev/null 2>&1

log() { echo "[backup] $(date -u +%H:%M:%S) $*"; }

log "Starting pg_dump of ${PGDATABASE}@${PGHOST}..."

TMPFILE=$(mktemp -t backup-XXXXXX)
trap 'rm -f "$TMPFILE"' EXIT

pg_dump -Fc --no-owner --no-privileges > "$TMPFILE"
SIZE=$(wc -c < "$TMPFILE" | tr -d ' ')
log "Dump complete: ${SIZE} bytes"

# Upload daily
mc cp "$TMPFILE" "s3bak/${BACKUP_S3_BUCKET}/daily/${FILENAME}" >/dev/null
log "Uploaded daily/${FILENAME}"

# On Sundays (day 7), also copy as weekly
if [ "$DAY_OF_WEEK" = "7" ]; then
  mc cp "$TMPFILE" "s3bak/${BACKUP_S3_BUCKET}/weekly/${FILENAME}" >/dev/null
  log "Uploaded weekly/${FILENAME} (Sunday)"
fi

# Retention: prune old daily backups
log "Pruning daily backups (retain last ${RETAIN_DAILY})..."
mc ls "s3bak/${BACKUP_S3_BUCKET}/daily/" --json 2>/dev/null \
  | grep '"key"' \
  | sed 's/.*"key":"\([^"]*\)".*/\1/' \
  | sort -r \
  | tail -n +$((RETAIN_DAILY + 1)) \
  | while read -r key; do
      mc rm "s3bak/${BACKUP_S3_BUCKET}/daily/${key}" >/dev/null 2>&1 && \
        log "  Removed daily/${key}"
    done

# Retention: prune old weekly backups
log "Pruning weekly backups (retain last ${RETAIN_WEEKLY})..."
mc ls "s3bak/${BACKUP_S3_BUCKET}/weekly/" --json 2>/dev/null \
  | grep '"key"' \
  | sed 's/.*"key":"\([^"]*\)".*/\1/' \
  | sort -r \
  | tail -n +$((RETAIN_WEEKLY + 1)) \
  | while read -r key; do
      mc rm "s3bak/${BACKUP_S3_BUCKET}/weekly/${key}" >/dev/null 2>&1 && \
        log "  Removed weekly/${key}"
    done

log "Done."
