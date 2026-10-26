set dotenv-load := true
set shell := ["bash", "-cu"]

# Compose lives at the repo root; every recipe assumes cwd is there.
# Long multi-step bash blocks live under scripts/ so this file stays a
# thin dispatcher (you can scan it top-to-bottom in 30 seconds).

compose := "docker compose"

# List available recipes
default:
    @just --list

# ----------------------------------------------------------------------
# Build / deploy
# ----------------------------------------------------------------------

# Runtime and YouTube builds use immutable SemVer tags. Other targets retain no_cache.
# Usage: just build [yoink|gateway|frontend|nginx|tg|backup|all] [no_cache=1]
build target="all" no_cache="":
    @scripts/build.sh "{{target}}" "{{no_cache}}"

# Download a published release without starting or replacing containers.
pull-images:
    node scripts/container-images.mjs pull

# Create host-side data directories required by volume mounts
data-dirs:
    mkdir -p data/cookies data/kasm/profile/chromium
    sudo -n install -d -m 0700 -o 101 -g 101 data/tg-bot-api
    sudo -n install -d -m 0700 -o 10001 -g 10001 data/native data/native/work data/native/work/downloads data/native-backups data/native-restore
    sudo -n setfacl -m u:10001:rx,d:u:10001:rx data/tg-bot-api

# Start all services (or a single one). Does NOT rebuild; use `rebuild`.
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

# `docker compose restart` sends SIGTERM and starts the same container ID
# against the same image, so a freshly built `:latest` is silently ignored
# (we hit this 2026-06-06 chasing a 'code didn't apply' ghost - the
# container was still running the previous image). `up -d --force-recreate`
# always rebuilds the container from the current image tag, so a new
# build actually lands.
#
# Recreate a running container from the CURRENT image. NOT compose restart.
restart service:
    {{compose}} up -d --force-recreate --no-deps {{service}}

# Build and deploy. Changed runtime inputs require a backend/VERSION bump; YouTube inputs require a new yoink_youtube_image tag.
# Usage: just rebuild [yoink|gateway|yoink-frontend|fe|yoink-nginx|nginx|tg-bot-api|tg|yoink-backup|backup|all] [no_cache=1]
rebuild service="" no_cache="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{service}}" in
      ""|all)              just build all      {{no_cache}} && {{compose}} up -d --no-build --wait --wait-timeout 120 native-youtube-auth-gateway && just --justfile backend/justfile release-deploy && {{compose}} up -d --no-deps yoink-frontend yoink-nginx ;;
      yoink)               just build yoink    {{no_cache}} && just --justfile backend/justfile release-deploy ;;
      gateway)             just build gateway  {{no_cache}} && {{compose}} up -d --no-deps --wait --wait-timeout 120 native-youtube-auth-gateway ;;
      yoink-frontend|fe)   just build frontend {{no_cache}} && {{compose}} up -d --force-recreate --no-deps yoink-frontend ;;
      yoink-nginx|nginx)   just build nginx    {{no_cache}} && {{compose}} up -d --force-recreate --no-deps yoink-nginx ;;
      tg-bot-api|tg)       just build tg       {{no_cache}} && {{compose}} up -d --force-recreate --no-deps tg-bot-api ;;
      yoink-backup|backup) just build backup   {{no_cache}} && {{compose}} up -d --force-recreate --no-deps yoink-backup ;;
      *) echo "Unknown service: {{service}}" >&2; exit 1 ;;
    esac

# Alias over `rebuild` so 'just deploy yoink' reads naturally in runbooks.
deploy service="": (rebuild service)

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

# ----------------------------------------------------------------------
# Database
# ----------------------------------------------------------------------

# Alembic ops via the yoink container. The working tree is bind-mounted
# over /app/src so new revision files apply without a rebuild.
# Usage: just migrate [up|down|current|history|create "msg"|check]
migrate action="up" message="":
    @scripts/migrate.sh "{{action}}" "{{message}}"

# Open psql shell
psql:
    {{compose}} exec yoink-postgres psql -U yoink -d yoink

# ----------------------------------------------------------------------
# Operations
# ----------------------------------------------------------------------

# Exec into a container shell (default: yoink)
shell service="yoink":
    {{compose}} exec {{service}} sh

# Cookie-extraction browser (Kasm profile persisted in data/kasm/).
# Usage: just browser [up|down|logs]
browser action="up":
    @scripts/browser.sh "{{action}}"

# Configure SOCKS5 proxy for bot + user sessions in tg-bot-api.
# One-shot: runs against the live tg-bot-api container, not a build step.
proxy-init:
    docker run --rm \
        --network yoink \
        --add-host host.docker.internal:host-gateway \
        -e bot_token="${bot_token}" \
        -e BOT_API_URL="http://tg-bot-api:8082" \
        -e socks5_host="host.docker.internal" \
        -e socks5_port="1080" \
        -e USER_TOKEN_FILE="/data/tg-bot-api/user.token" \
        -v "$(pwd)/docker/init-proxy.sh:/init-proxy.sh:ro" \
        -v "$(pwd)/data/tg-bot-api:/data/tg-bot-api:ro" \
        busybox sh /init-proxy.sh

# tg-bot-api user-mode session.
# Usage: just tg [login +79001234567|status|logout]
tg action="status" phone="":
    @scripts/tg-login.sh "{{action}}" "{{phone}}"

# Backup container control. Lives behind the `backup` compose profile.
# Usage: just backup [run|restore [target]|up|down|logs]
backup action="run" target="latest":
    @scripts/backup.sh "{{action}}" "{{target}}"

# ----------------------------------------------------------------------
# Lint / verify / test
# ----------------------------------------------------------------------

# Host-side smoke check (ruff E9 + F8x). Pre-build wall, fail-fast.
verify-host:
    @scripts/verify-host.sh

# Usage: just lint                  full repo (src + plugins)
#        just lint --fix            autofix both trees
#        just lint src/yoink        scope to a path
# Full ruff lint inside a throwaway container. Forwards args to ruff.
lint:
    just --justfile backend/justfile lint

lint-py *args="":
    @scripts/lint.sh {{args}}

# Python verify: ruff lint + pytest. Fails fast on first red step.
# Usage: just verify-py [path]
verify-py *args="src/tests":
    just lint-py
    just test-legacy {{args}}

# Translation coverage and regression tests across core + plugin frontends.
verify-i18n:
    cd frontend && npm run test:i18n && npm run check:i18n

test-inbox-frontend:
    node --test plugins/yoink-inbox/frontend/tests/frontend.test.mjs

test-insight-frontend:
    node --test plugins/yoink-insight/frontend/tests/credentials.test.mjs

# Frontend verify: translations, Inbox/Insight regressions and integrated tsc.
verify-fe *args="":
    just verify-i18n
    just test-inbox-frontend
    just test-insight-frontend
    just tsc {{args}}

# Native backend, deployment contracts and frontend verification.
verify:
    just --justfile backend/justfile verify
    node --test scripts/test-native-deployment.mjs
    just test-publication
    just verify-fe

test-publication:
    node --test scripts/container-images.test.mjs scripts/registry-image.test.mjs
    node --check scripts/test-container-image.mjs

# Verify committed HEAD with public submodule remotes, not uncommitted changes.
test-clean-checkout:
    node scripts/test-clean-checkout.mjs

# Run tests inside a container against yoink_test DB in the existing postgres.
# Usage: just test [path]
# The default is the core suite. Plugin directories without tests are not valid
# pytest targets and are intentionally not used as the default.
test:
    just --justfile backend/justfile test

test-legacy *args="src/tests":
    #!/usr/bin/env bash
    set -euo pipefail
    # Ensure yoink_test database exists; cheap if it already does.
    {{compose}} exec -T yoink-postgres psql -U yoink -d postgres -tAc \
        "SELECT 1 FROM pg_database WHERE datname='yoink_test'" | grep -q 1 \
        || {{compose}} exec -T yoink-postgres psql -U yoink -d postgres -c \
            "CREATE DATABASE yoink_test OWNER yoink;"
    docker run --rm \
        --network yoink \
        -v "$(pwd)/src:/app/src:ro" \
        -v "$(pwd)/plugins:/app/plugins:ro" \
        -v "$(pwd)/pyproject.toml:/app/pyproject.toml:ro" \
        yoink/yoink:latest \
        sh -c '
          uv pip install --system pytest pytest-asyncio -q >/dev/null
          status=0
          python -m pytest {{args}} -v --tb=short || status=$?
          if [ "$status" -eq 5 ]; then
            echo "No tests collected for: {{args}} (treated as success)"
            exit 0
          fi
          exit "$status"
        '

# Full reset: down -v + up + migrate. Wipes volumes; dev only, NEVER prod.
reset:
    {{compose}} down -v
    just up
    sleep 5
    just migrate up

# ----------------------------------------------------------------------
# Cleanup
# ----------------------------------------------------------------------

# Remove __pycache__ / *.pyc / *.egg-info from src/ only.
clean-pyc:
    find src -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
    find src -name "*.pyc" -delete 2>/dev/null || true
    find src -name "*.egg-info" -type d -exec rm -rf {} + 2>/dev/null || true

# Remove all host-side build artifacts (node_modules, pycache, dist, .venv).
clean:
    @scripts/clean.sh

# ----------------------------------------------------------------------
# Frontend dev
# ----------------------------------------------------------------------

# Vite dev server with HMR on the host (http://localhost:5173).
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -d frontend/node_modules ] || [ "$(stat -c %U frontend/node_modules 2>/dev/null)" = "root" ]; then
        echo "node_modules missing or owned by root - run: just setup-dev"
        exit 1
    fi
    echo "Starting Vite dev server at http://localhost:5173"
    cd frontend && npm run dev

# Install host-side node_modules + plugin symlinks for local Vite dev.
setup-dev:
    @scripts/dev-setup.sh

# Run TypeScript type-check using host node_modules (requires just setup-dev).
tsc *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    cd frontend && npx tsc --noEmit {{args}}

# Run npm against the frontend without a host-side node install.
# chown trail at the end keeps frontend/package*.json editable without sudo
# after a container-managed `npm install foo`.
# Usage: just npm install --save-dev some-pkg
npm *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    docker run --rm \
        -v "$(pwd)/frontend:/app/frontend" \
        -w /app/frontend \
        node:22-alpine \
        sh -c "npm {{args}}"
    sudo chown -R "$(id -u):$(id -g)" frontend/package.json frontend/package-lock.json 2>/dev/null || true

# Install a shadcn component into the frontend (runs in a temp container).
# Files land under frontend/src/components/ui owned by the container user;
# chown back so subsequent edits don't need sudo.
# Usage: just shadcn add progress
shadcn *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    docker run --rm \
        -v "$(pwd)/frontend:/app/frontend" \
        -w /app/frontend \
        node:22-alpine \
        sh -c "npm install -g shadcn@latest 2>/dev/null; npx shadcn {{args}}"
    sudo chown -R "$(id -u):$(id -g)" frontend/src/components/ui frontend/package.json frontend/package-lock.json 2>/dev/null || true

# ----------------------------------------------------------------------
# Submodules / git
# ----------------------------------------------------------------------

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
