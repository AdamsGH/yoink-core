set dotenv-load := true
set shell := ["bash", "-cu"]

image_yoink    := "yoink/yoink:latest"
image_frontend := "yoink/frontend:latest"
image_nginx    := "yoink/nginx:latest"
image_tg       := "yoink/tg-bot-api:latest"

compose    := "docker compose"
tg_url     := "http://127.0.0.1:8082"
tg_token   := "data/tg-bot-api/user.token"
tg_noproxy := "127.0.0.1,localhost"

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
        docker build -f docker/Dockerfile --progress=plain \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_yoink}}    .
        ;;
      frontend)
        docker build -f docker/Dockerfile.frontend --progress=plain \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_frontend}} .
        ;;
      nginx)
        docker build -f docker/Dockerfile.nginx --progress=plain \
          -t {{image_nginx}} .
        ;;
      tg)
        docker build -f docker/Dockerfile.tg-bot-api --progress=plain -t {{image_tg}} .
        ;;
      backup)
        docker build -f docker/Dockerfile.backup --progress=plain -t yoink/backup:latest .
        ;;
      all)
        docker build -f docker/Dockerfile --progress=plain \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_yoink}}    .
        docker build -f docker/Dockerfile.frontend --progress=plain \
          --build-arg CACHE_BUST=$(git rev-parse HEAD) \
          -t {{image_frontend}} .
        docker build -f docker/Dockerfile.nginx --progress=plain \
          -t {{image_nginx}} .
        ;;
      *)
        echo "Unknown target: {{target}}. Use: yoink | frontend | nginx | tg | backup | all"
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

# Manage the user-mode session. Usage: just tg [login +79001234567|status|logout]
tg action="status" phone="":
    #!/usr/bin/env bash
    set -euo pipefail
    TG_URL="{{tg_url}}"
    no_proxy="{{tg_noproxy}}"
    NO_PROXY="{{tg_noproxy}}"
    TOKEN_FILE="{{tg_token}}"
    PY="docker/tg-login.py"
    case "{{action}}" in
      login)
        if [ -z "{{phone}}" ]; then
            echo "Usage: just tg login +79001234567"
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
        echo "[tg] Requesting code for {{phone}} ..."
        curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/userLogin" \
            -d "phone_number={{phone}}" > "$RESP_FILE" 2>/dev/null &
        LOGIN_PID=$!
        echo "[tg] Waiting for session token in user_db.binlog..."
        new_token=$(sudo python3 "$PY" watch-token "$DB_PATH" "$PHONE_DIGITS" "$KNOWN_FILE" 2>/dev/null)
        if [ -n "$new_token" ]; then
            echo "[tg] Got token, configuring SOCKS5 proxy..."
            python3 "$PY" proxy-add "$TG_URL" "$new_token" "host.docker.internal" "1080"
        else
            echo "[tg] WARNING: could not detect token early, proxy may not be set"
        fi
        wait "$LOGIN_PID" || true
        resp=$(cat "$RESP_FILE")
        rm -f "$RESP_FILE" "$KNOWN_FILE"
        state=$(echo "$resp" | python3 "$PY" state)
        token=$(echo "$resp" | python3 "$PY" token)
        if [ "$state" = "ready" ]; then
            echo "[tg] Already authorized."
            printf '%s' "$token" > "$TOKEN_FILE"
            chmod 600 "$TOKEN_FILE"
            echo "[tg] Token saved to $TOKEN_FILE"
            exit 0
        fi
        if [ "$state" != "wait_code" ]; then
            echo "[tg] Unexpected state: $state"
            echo "$resp"
            exit 1
        fi
        read -rp "[tg] Enter the code from Telegram: " code
        resp=$(curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/user${token}/authcode" -d "code=${code}" || true)
        state=$(echo "$resp" | python3 "$PY" state)
        if [ "$state" = "wait_password" ]; then
            read -rsp "[tg] Enter your 2FA password: " pwd; echo
            resp=$(curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/user${token}/authpassword" -d "password=${pwd}" || true)
            state=$(echo "$resp" | python3 "$PY" state)
        fi
        if [ "$state" = "wait_registration" ]; then
            read -rp "[tg] First name: " fname
            read -rp "[tg] Last name:  " lname
            resp=$(curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/user${token}/registeruser" \
                -d "first_name=${fname}" -d "last_name=${lname}" || true)
            state=$(echo "$resp" | python3 "$PY" state)
        fi
        if [ "$state" = "ready" ]; then
            printf '%s' "$token" > "$TOKEN_FILE"
            chmod 600 "$TOKEN_FILE"
            echo "[tg] Authorized. Token saved to $TOKEN_FILE"
        else
            echo "[tg] Authorization failed. State: $state"
            echo "$resp"
            exit 1
        fi
        ;;
      status)
        if [ ! -f "$TOKEN_FILE" ]; then
            echo "[tg] No token file. Run: just tg login +<phone>"
            exit 1
        fi
        token=$(cat "$TOKEN_FILE")
        echo "[tg] Token: ${token:0:20}...(redacted)"
        resp=$(curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/user${token}/getMe" || echo '{"ok":false}')
        python3 "$PY" status <<< "$resp"
        ;;
      logout)
        if [ ! -f "$TOKEN_FILE" ]; then
            echo "[tg] No token file."
            exit 0
        fi
        token=$(cat "$TOKEN_FILE")
        curl -sf --noproxy '{{tg_noproxy}}' "${TG_URL}/user${token}/logout" > /dev/null && echo "[tg] Logged out."
        rm -f "$TOKEN_FILE"
        echo "[tg] Token file removed."
        ;;
      *)
        echo "Usage: just tg [login +<phone>|status|logout]"
        exit 1
        ;;
    esac

# Manage backups. Usage: just backup [run|restore [target]|up|down|logs]
backup action="run" target="latest":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{action}}" in
      run)
        {{compose}} --profile backup run --rm --entrypoint /backup.sh yoink-backup
        ;;
      restore)
        {{compose}} --profile backup run --rm --entrypoint /restore.sh yoink-backup "{{target}}"
        ;;
      up)
        {{compose}} --profile backup up -d yoink-backup
        ;;
      down)
        {{compose}} --profile backup stop yoink-backup
        ;;
      logs)
        {{compose}} --profile backup logs -f --tail=50 yoink-backup
        ;;
      *)
        echo "Usage: just backup [run|restore [target]|up|down|logs]"
        exit 1
        ;;
    esac

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

# Remove all host-side build artifacts (node_modules, pycache, .venv).
# All building and type-checking must happen inside containers.
clean:
    #!/usr/bin/env bash
    set -euo pipefail
    find . -name "node_modules" -not -path "*/.git/*" -prune -exec rm -rf {} + 2>/dev/null || true
    find . -type d -name __pycache__ -not -path "*/.git/*" -exec rm -rf {} + 2>/dev/null || true
    find . -name "*.pyc" -not -path "*/.git/*" -delete 2>/dev/null || true
    find . -name "*.egg-info" -type d -not -path "*/.git/*" -exec rm -rf {} + 2>/dev/null || true
    rm -rf .venv frontend/dist
    echo "Host artifacts cleaned."

# Run Vite dev server with HMR on the host. Access at http://localhost:5173
# Prerequisites: run `just setup-dev` once to install node_modules on host.
# API is proxied to the running yoink backend at port 8000.
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -d frontend/node_modules ] || [ "$(stat -c %U frontend/node_modules 2>/dev/null)" = "root" ]; then
        echo "node_modules missing or owned by root - run: just setup-dev"
        exit 1
    fi
    echo "Starting Vite dev server at http://localhost:5173"
    cd frontend && npm run dev

# Install host-side node_modules and plugin symlinks for local dev (run once).
# Also run after `just build frontend` or `just tsc` to fix root-owned files.
setup-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    sudo chown -R "$(id -u):$(id -g)" frontend/node_modules 2>/dev/null || true
    rm -rf frontend/node_modules/.vite
    cd frontend && npm install
    for p in yoink-dl yoink-stats yoink-insight; do
        rm -rf "$(pwd)/../plugins/$p/frontend/node_modules"
        ln -sf "$(pwd)/node_modules" "$(pwd)/../plugins/$p/frontend/node_modules"
        echo "linked $p"
    done
    echo "Dev setup complete. Run: just dev"

# Run TypeScript type-check using host node_modules (requires just setup-dev).
tsc *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    cd frontend && npx tsc --noEmit {{args}}

# Run npm commands for frontend (e.g. just npm 'add some-pkg').
npm *args="":
    cd frontend && npm {{args}}

# Install a shadcn component into the frontend (runs inside a temp container).
# Usage: just shadcn add progress
shadcn *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    docker run --rm \
        -v "$(pwd)/frontend:/app/frontend" \
        -w /app/frontend \
        node:22-alpine \
        sh -c "npm install -g shadcn@latest 2>/dev/null; npx shadcn {{args}}"

# Show git log across core + all plugin submodules
log *args="--oneline -20":
    #!/usr/bin/env bash
    echo "=== core ==="
    git log {{args}}
    for p in plugins/*/; do
        [ -d "$p/.git" ] || continue
        echo "=== $p ==="
        git -C "$p" log {{args}}
    done

# Pull latest for core and all submodules
pull:
    git pull
    git submodule update --remote --merge

# Show submodule status
submodules:
    git submodule status
