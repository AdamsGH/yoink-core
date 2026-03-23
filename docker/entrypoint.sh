#!/bin/sh
set -e
export PYTHONDONTWRITEBYTECODE=1

SHOULD_MIGRATE=0
for arg in "$@"; do
    case "$arg" in
        yoink-api|yoink-combined) SHOULD_MIGRATE=1 ;;
    esac
done

if [ "$SHOULD_MIGRATE" = "1" ]; then
    echo "[entrypoint] Checking Alembic state..."

    STAMP_NEEDED=$(python - <<'EOF'
import os, sys
try:
    import psycopg
    url = (os.environ.get('DATABASE_URL') or os.environ.get('database_url', '')) \
        .replace('postgresql+asyncpg://', 'postgresql://') \
        .replace('postgresql+psycopg://', 'postgresql://')
    with psycopg.connect(url) as conn:
        with conn.cursor() as cur:
            cur.execute("SELECT 1 FROM information_schema.tables WHERE table_name='alembic_version'")
            exists = cur.fetchone() is not None
    print('no' if exists else 'yes')
except Exception as e:
    print('error:' + str(e), file=sys.stderr)
    sys.exit(1)
EOF
)

    if [ "$STAMP_NEEDED" = "yes" ]; then
        echo "[entrypoint] First run - stamping alembic at head..."
        alembic -c /app/src/db/alembic.ini stamp head
    elif [ "$STAMP_NEEDED" = "no" ]; then
        echo "[entrypoint] Running Alembic migrations..."
        alembic -c /app/src/db/alembic.ini upgrade head
    else
        echo "[entrypoint] Could not check alembic state, skipping"
    fi
fi

echo "[entrypoint] Starting: $*"
exec "$@"
