#!/usr/bin/env bash
# Backup container control. Lives behind the `backup` compose profile so
# the service doesn't auto-start on `just up` - it's a cron container
# meant to wake on schedule, not run 24/7.
#
# Usage: scripts/backup.sh <action> [target]
#   run      execute /backup.sh once and exit
#   restore  restore from `target` (default: latest)
#   up       start the cron container (background)
#   down     stop it
#   logs     tail logs

set -euo pipefail

action="${1:-run}"
target="${2:-latest}"

case "$action" in
  run)
    docker compose --profile backup run --rm --entrypoint /backup.sh yoink-backup
    ;;
  restore)
    docker compose --profile backup run --rm --entrypoint /restore.sh yoink-backup "$target"
    ;;
  up)
    docker compose --profile backup up -d yoink-backup
    ;;
  down)
    docker compose --profile backup stop yoink-backup
    ;;
  logs)
    docker compose --profile backup logs -f --tail=50 yoink-backup
    ;;
  *)
    echo "Usage: $0 [run|restore [target]|up|down|logs]" >&2
    exit 1
    ;;
esac
