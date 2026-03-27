set dotenv-load := true
set shell := ["bash", "-cu"]

image_yoink    := "yoink/yoink:latest"
image_frontend := "yoink/frontend:latest"
image_tg       := "yoink/tg-bot-api:latest"

compose := "docker compose"

# List available recipes
default:
    @just --list

# Build images. Context is yoink-core root (plugins via submodules).
# Usage: just build [yoink|frontend|tg|backup|all]
build target="all":
    #!/usr/bin/env bash
    set -euo pipefail
    cd "$(dirname "$(just --justfile)")"
    case "{{target}}" in
      yoink)
        docker build -f docker/Dockerfile           -t {{image_yoink}}    .
        ;;
      frontend)
        docker build -f docker/Dockerfile.frontend \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_frontend}} .
        ;;
      tg)
        docker build -f docker/Dockerfile.tg-bot-api -t {{image_tg}}      .
        ;;
      backup)
        docker build -f docker/Dockerfile.backup     -t yoink/backup:latest .
        ;;
      all)
        docker build -f docker/Dockerfile            -t {{image_yoink}}    .
        docker build -f docker/Dockerfile.frontend \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_frontend}} .
        ;;
      *)
        echo "Unknown target: {{target}}. Use: yoink | frontend | tg | backup | all"
        exit 1
        ;;
    esac
    docker image prune -f

# Create host-side data directories required by volume mounts
data-dirs:
    mkdir -p data/cookies data/kasm/profile/chromium data/tg-bot-api
    chmod 700 data/tg-bot-api

# Start all services (or a single one: just up yoink)
up service="":
    #!/usr/bin/env bash
    if [ -z "{{service}}" ]; then
        {{compose}} up -d
    else
        {{compose}} up -d {{service}}
    fi

# Stop all services
down:
    {{compose}} down

# Restart a service
restart service:
    {{compose}} restart {{service}}

# Follow logs (all or a single service)
logs service="":
    #!/usr/bin/env bash
    if [ -z "{{service}}" ]; then
        {{compose}} logs -f --tail=100
    else
        {{compose}} logs -f --tail=100 {{service}}
    fi

# Show running containers
ps:
    {{compose}} ps

# Database migrations via alembic inside the api container
migrate action="up" message="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{action}}" in
      up)
        {{compose}} run --rm yoink sh -c "cd /app/src/db && alembic upgrade head"
        ;;
      down)
        {{compose}} run --rm yoink sh -c "cd /app/src/db && alembic downgrade -1"
        ;;
      current)
        {{compose}} run --rm yoink sh -c "cd /app/src/db && alembic current"
        ;;
      history)
        {{compose}} run --rm yoink sh -c "cd /app/src/db && alembic history"
        ;;
      create)
        if [ -z "{{message}}" ]; then
          echo "Usage: just migrate create \"message\""
          exit 1
        fi
        {{compose}} run --rm yoink sh -c "cd /app/src/db && alembic revision --autogenerate -m '{{message}}'"
        ;;
      *)
        echo "Usage: just migrate [up|down|current|history|create \"msg\"]"
        exit 1
        ;;
    esac

# Open psql shell
psql:
    {{compose}} exec yoink-postgres psql -U yoink -d yoink

# Exec into a container shell (default: yoink)
shell service="yoink":
    {{compose}} exec {{service}} sh

# Start browser container for cookie extraction (profile persisted in data/kasm/)
browser action="up":
    #!/usr/bin/env bash
    case "{{action}}" in
      up)
        {{compose}} --profile cookies up -d yoink-browser
        echo "Browser: http://$(hostname):6902"
        ;;
      down)
        {{compose}} --profile cookies down yoink-browser
        ;;
      logs)
        {{compose}} --profile cookies logs -f yoink-browser
        ;;
      *)
        echo "Usage: just browser [up|down|logs]"
        exit 1
        ;;
    esac

# Configure SOCKS5 proxy for bot and user sessions in tg-bot-api
proxy-init:
    docker run --rm \
        --network yoink \
        --add-host host.docker.internal:host-gateway \
        -e bot_token="${bot_token}" \
        -e BOT_API_URL="http://yoink-tg-bot-api:8082" \
        -e socks5_host="host.docker.internal" \
        -e socks5_port="1080" \
        -e USER_TOKEN_FILE="/data/tg-bot-api/user.token" \
        -v "$(pwd)/docker/init-proxy.sh:/init-proxy.sh:ro" \
        -v "$(pwd)/data/tg-bot-api:/data/tg-bot-api:ro" \
        busybox sh /init-proxy.sh

# Interactive Telegram user-mode login.
# Stores session token in data/tg-bot-api/user.token.
# Usage: just tg-login +79001234567
tg-login phone="":
    #!/usr/bin/env bash
    set -euo pipefail
    TG_URL="http://127.0.0.1:8082"
    no_proxy="127.0.0.1,localhost"
    NO_PROXY="127.0.0.1,localhost"
    TOKEN_FILE="data/tg-bot-api/user.token"
    PY="docker/tg-login.py"

    if [ -z "{{phone}}" ]; then
        echo "Usage: just tg-login +79001234567"
        exit 1
    fi

    mkdir -p data/tg-bot-api
    chmod 700 data/tg-bot-api

    PHONE_DIGITS="{{phone}}"
    PHONE_DIGITS="${PHONE_DIGITS//+/}"
    DB_PATH="data/tg-bot-api/user_db.binlog"
    KNOWN_FILE=$(mktemp)
    RESP_FILE=$(mktemp)

    sudo python3 "$PY" watch-token "$DB_PATH" "$PHONE_DIGITS" snap 2>/dev/null > "$KNOWN_FILE" || true

    echo "[tg-login] Requesting code for {{phone}} ..."
    curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/userLogin" \
        -d "phone_number={{phone}}" > "$RESP_FILE" 2>/dev/null &
    LOGIN_PID=$!

    echo "[tg-login] Waiting for session token in user_db.binlog..."
    new_token=$(sudo python3 "$PY" watch-token "$DB_PATH" "$PHONE_DIGITS" "$KNOWN_FILE" 2>/dev/null)

    if [ -n "$new_token" ]; then
        echo "[tg-login] Got token, configuring SOCKS5 proxy..."
        python3 "$PY" proxy-add "$TG_URL" "$new_token" "host.docker.internal" "1080"
    else
        echo "[tg-login] WARNING: could not detect token early, proxy may not be set"
    fi

    wait "$LOGIN_PID" || true
    resp=$(cat "$RESP_FILE")
    rm -f "$RESP_FILE" "$KNOWN_FILE"

    state=$(echo "$resp" | python3 "$PY" state)
    token=$(echo "$resp" | python3 "$PY" token)

    if [ "$state" = "ready" ]; then
        echo "[tg-login] Already authorized."
        printf '%s' "$token" > "$TOKEN_FILE"
        chmod 600 "$TOKEN_FILE"
        echo "[tg-login] Token saved to $TOKEN_FILE"
        exit 0
    fi

    if [ "$state" != "wait_code" ]; then
        echo "[tg-login] Unexpected state: $state"
        echo "$resp"
        exit 1
    fi

    read -rp "[tg-login] Enter the code from Telegram: " code
    resp=$(curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/user${token}/authcode" -d "code=${code}" || true)
    state=$(echo "$resp" | python3 "$PY" state)

    if [ "$state" = "wait_password" ]; then
        read -rsp "[tg-login] Enter your 2FA password: " pwd; echo
        resp=$(curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/user${token}/authpassword" -d "password=${pwd}" || true)
        state=$(echo "$resp" | python3 "$PY" state)
    fi

    if [ "$state" = "wait_registration" ]; then
        read -rp "[tg-login] First name: " fname
        read -rp "[tg-login] Last name:  " lname
        resp=$(curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/user${token}/registeruser" \
            -d "first_name=${fname}" -d "last_name=${lname}" || true)
        state=$(echo "$resp" | python3 "$PY" state)
    fi

    if [ "$state" = "ready" ]; then
        printf '%s' "$token" > "$TOKEN_FILE"
        chmod 600 "$TOKEN_FILE"
        echo "[tg-login] Authorized. Token saved to $TOKEN_FILE"
    else
        echo "[tg-login] Authorization failed. State: $state"
        echo "$resp"
        exit 1
    fi

# Show current user-mode auth state
tg-status:
    #!/usr/bin/env bash
    TG_URL="http://127.0.0.1:8082"
    no_proxy="127.0.0.1,localhost"
    NO_PROXY="127.0.0.1,localhost"
    TOKEN_FILE="data/tg-bot-api/user.token"
    if [ ! -f "$TOKEN_FILE" ]; then
        echo "[tg-status] No token file. Run: just tg-login +<phone>"
        exit 1
    fi
    token=$(cat "$TOKEN_FILE")
    echo "[tg-status] Token: ${token:0:20}...(redacted)"
    resp=$(curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/user${token}/getMe" || echo '{"ok":false}')
    python3 docker/tg-login.py status <<< "$resp"

# Log out and remove the user session token
tg-logout:
    #!/usr/bin/env bash
    TG_URL="http://127.0.0.1:8082"
    no_proxy="127.0.0.1,localhost"
    NO_PROXY="127.0.0.1,localhost"
    TOKEN_FILE="data/tg-bot-api/user.token"
    if [ ! -f "$TOKEN_FILE" ]; then
        echo "[tg-logout] No token file."
        exit 0
    fi
    token=$(cat "$TOKEN_FILE")
    curl -sf --noproxy '127.0.0.1,localhost' "${TG_URL}/user${token}/logout" > /dev/null && echo "[tg-logout] Logged out."
    rm -f "$TOKEN_FILE"
    echo "[tg-logout] Token file removed."

# Run a one-shot backup now (requires backup_s3_* env vars)
backup:
    {{compose}} --profile backup run --rm --entrypoint /backup.sh yoink-backup

# Restore from S3 backup. Usage: just restore [latest | daily/yoink-*.dump | list]
restore target="latest":
    {{compose}} --profile backup run --rm --entrypoint /restore.sh yoink-backup "{{target}}"

# Start the backup cron container
backup-up:
    {{compose}} --profile backup up -d yoink-backup

# Stop the backup cron container
backup-down:
    {{compose}} --profile backup down yoink-backup

# Follow backup container logs
backup-logs:
    {{compose}} --profile backup logs -f --tail=50 yoink-backup

# Run tests inside a container against yoink_test DB in the existing postgres.
# Usage: just test [path] (e.g. just test src/tests/test_rbac.py)
test *args="src/tests":
    #!/usr/bin/env bash
    set -euo pipefail
    # Ensure yoink_test database exists
    {{compose}} exec -T yoink-postgres psql -U yoink -d postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname='yoink_test'" | grep -q 1 \
        || {{compose}} exec -T yoink-postgres psql -U yoink -d postgres -c \
            "CREATE DATABASE yoink_test OWNER yoink;"
    docker run --rm \
        --network yoink \
        -v "$(pwd)/src:/app/src:ro" \
        -v "$(pwd)/pyproject.toml:/app/pyproject.toml:ro" \
        yoink/yoink:latest \
        sh -c "uv pip install --system pytest pytest-asyncio -q && python -m pytest {{args}} -v --tb=short"

# Full reset: stop, remove volumes, restart, migrate
reset:
    {{compose}} down -v
    just up
    sleep 5
    just migrate up

# Remove Python cache artifacts from src/
clean-pyc:
    find src -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
    find src -name "*.pyc" -delete 2>/dev/null || true
    find src -name "*.egg-info" -type d -exec rm -rf {} + 2>/dev/null || true
