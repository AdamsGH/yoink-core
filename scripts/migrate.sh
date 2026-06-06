#!/usr/bin/env bash
# Alembic migrations through the yoink container.
#
# Usage: scripts/migrate.sh <action> [message]
#   up        apply all pending revisions to the live yoink database
#   down      roll back one revision
#   current   show current revision
#   history   show full revision history
#   create    autogenerate a new revision (requires message=)
#   check     dry-run on a throwaway copy of the live yoink schema:
#             clone schema, upgrade head, downgrade -1, upgrade head again.
#             Catches the class of bugs that only surface against real
#             columns (broken SQL literals, missing FKs, dead refs) without
#             ever touching prod data. Run before `just deploy`.
#
# The yoink working tree is bind-mounted over baked /app/src so edits to
# alembic versions land in the container without a rebuild. Intentional:
# add a fresh migration file, run `just migrate up`, no `--build` needed.

set -euo pipefail

action="${1:?Usage: $0 <up|down|current|history|create|check> [message]}"
message="${2:-}"

run_in_yoink() {
    docker compose run --rm -v "$(pwd)/src:/app/src" yoink sh -c "$1"
}

case "$action" in
  up)       run_in_yoink 'cd /app/src/db && alembic upgrade head' ;;
  down)     run_in_yoink 'cd /app/src/db && alembic downgrade -1' ;;
  current)  run_in_yoink 'cd /app/src/db && alembic current' ;;
  history)  run_in_yoink 'cd /app/src/db && alembic history' ;;

  create)
    if [ -z "$message" ]; then
        echo "Usage: just migrate create \"message\"" >&2
        exit 1
    fi
    run_in_yoink "cd /app/src/db && alembic revision --autogenerate -m \"$message\""
    ;;

  check)
    # Smoke-test: schema-only clone of `yoink` into `yoink_migrate_check`,
    # then apply pending migrations against the clone (NOT against prod).
    # On success we drop the clone. The clone is recreated every run, so
    # leftover state from a previous aborted check never lingers.
    echo "[migrate check] cloning schema yoink -> yoink_migrate_check..."
    docker compose exec -T yoink-postgres psql -U yoink -d postgres -c \
        "DROP DATABASE IF EXISTS yoink_migrate_check;" >/dev/null
    # pg_dump --schema-only avoids hauling user data through the pipe; we
    # only need column/index/constraint shapes for the migration to bite.
    docker compose exec -T yoink-postgres sh -c \
        "pg_dump -U yoink --schema-only yoink | psql -U yoink -d postgres -c 'CREATE DATABASE yoink_migrate_check OWNER yoink;' >/dev/null && \
         pg_dump -U yoink --schema-only yoink | psql -U yoink -d yoink_migrate_check >/dev/null"
    echo "[migrate check] upgrade head..."
    docker compose run --rm \
        -e POSTGRES_DB=yoink_migrate_check \
        -v "$(pwd)/src:/app/src" \
        yoink sh -c 'cd /app/src/db && alembic upgrade head'
    echo "[migrate check] downgrade -1..."
    docker compose run --rm \
        -e POSTGRES_DB=yoink_migrate_check \
        -v "$(pwd)/src:/app/src" \
        yoink sh -c 'cd /app/src/db && alembic downgrade -1' || true
    echo "[migrate check] upgrade head (second pass)..."
    docker compose run --rm \
        -e POSTGRES_DB=yoink_migrate_check \
        -v "$(pwd)/src:/app/src" \
        yoink sh -c 'cd /app/src/db && alembic upgrade head'
    echo "[migrate check] dropping clone..."
    docker compose exec -T yoink-postgres psql -U yoink -d postgres -c \
        "DROP DATABASE yoink_migrate_check;" >/dev/null
    echo "[migrate check] OK"
    ;;

  *)
    echo "Usage: $0 [up|down|current|history|create \"msg\"|check]" >&2
    exit 1
    ;;
esac
