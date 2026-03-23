#!/bin/sh
# Entrypoint for the backup container in cron mode.
# Writes a crontab entry from BACKUP_CRON_SCHEDULE and runs crond in foreground.
# Pass env vars to the cron job via /etc/backup.env.

set -eu

SCHEDULE="${BACKUP_CRON_SCHEDULE:-0 3 * * *}"

env | grep -E '^(PG|BACKUP_S3|TZ)' > /etc/backup.env

echo "${SCHEDULE} . /etc/backup.env; /backup.sh >> /proc/1/fd/1 2>&1" \
  | crontab -

echo "[backup-cron] Schedule: ${SCHEDULE}"
echo "[backup-cron] Waiting for first run..."
exec crond -f -l 6
