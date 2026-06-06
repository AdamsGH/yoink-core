#!/usr/bin/env bash
# Host-side smoke checks that run BEFORE `docker build`.
#
# Catches the cheap class of failures (undefined names, syntax errors,
# bad f-strings) without uploading a docker context or starting a builder.
# Mirrors the rule from gateway's justfile: fail fast on garbage syntax
# before BuildKit even wakes up. The check itself runs inside the cached
# yoink image (one extra ruff install if the cache is cold) because
# AGENTS.md forbids running uv/python/pip on the host directly.
#
# Scope is intentionally narrow: only error-class ruff rules (E9, F8x).
# Full lint stays in `just lint`.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

if ! docker image inspect yoink/yoink:latest >/dev/null 2>&1; then
    echo "[verify-host] yoink/yoink:latest not built yet; skipping smoke checks."
    echo "[verify-host] First build will run lint inside the image anyway."
    exit 0
fi

docker run --rm \
    -v "$(pwd)/src:/app/src:ro" \
    -v "$(pwd)/plugins:/app/plugins:ro" \
    -v "$(pwd)/pyproject.toml:/app/pyproject.toml:ro" \
    -w /app \
    yoink/yoink:latest \
    sh -c "uv pip install --system ruff -q && ruff check --no-cache --select E9,F821,F822,F823,F9 src plugins"
