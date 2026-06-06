#!/usr/bin/env bash
# Ruff lint inside a throwaway container. Forwards every arg to ruff.
#
# Path-detection rule: any arg that does NOT start with `-` is treated as
# a path override and replaces the default `src plugins` target set.
# Flags (--fix, --select, ...) are forwarded and the default targets stay.
# This lets `just lint --fix` autofix both trees and `just lint src/yoink`
# scope to a single dir, without needing two separate recipes.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

args="$*"
has_path=0
for a in $args; do
    case "$a" in -*) ;; *) has_path=1; break ;; esac
done
targets="src plugins"
[ "$has_path" = "1" ] && targets=""

docker run --rm \
    -v "$(pwd)/src:/app/src:ro" \
    -v "$(pwd)/plugins:/app/plugins:ro" \
    -v "$(pwd)/pyproject.toml:/app/pyproject.toml:ro" \
    -w /app \
    yoink/yoink:latest \
    sh -c "uv pip install --system ruff -q && ruff check --no-cache $args $targets"
