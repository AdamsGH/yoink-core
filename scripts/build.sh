#!/usr/bin/env bash
# Build images with quiet success output and actionable failure logs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

target="${1:-all}"
no_cache="${2:-}"
case "$no_cache" in 1|true|yes|on) cache=(--no-cache) ;; "") cache=() ;; *) echo "Invalid no_cache value: $no_cache (use 1/true/yes/on)" >&2; exit 2 ;; esac
common=("${cache[@]}")
log=$(mktemp)
trap 'rm -f "$log"' EXIT

run_build() {
  local name="$1" dockerfile="$2" image="$3" context="${4:-.}" target="${5:-}"
  local target_args=()
  if [ -n "$target" ]; then target_args=(--target "$target"); fi
  printf 'Building %-10s' "$name"
  if docker build "${common[@]}" "${target_args[@]}" -f "$dockerfile" -t "$image" "$context" >"$log" 2>&1; then
    printf ' OK\n'
  else
    printf ' FAILED\n\n%s\n' "$(cat "$log")" >&2
    return 1
  fi
}

build_runtime() {
  if [ -n "$no_cache" ]; then
    echo 'Release images are immutable. Bump backend/VERSION instead of rebuilding a tag with no_cache.' >&2
    return 2
  fi
  just --justfile backend/justfile release-build
}

build_gateway() {
  if [ -n "$no_cache" ]; then
    echo 'YouTube images are immutable. Change yoink_youtube_image to a new SemVer instead of rebuilding with no_cache.' >&2
    return 2
  fi
  just --justfile backend/justfile build-youtube-services
}

case "$target" in
  yoink)    build_runtime ;;
  gateway)  build_gateway ;;
  frontend) run_build frontend docker/Dockerfile.frontend yoink/frontend:latest ;;
  nginx)    run_build nginx docker/Dockerfile.nginx yoink/nginx:latest ;;
  tg)       run_build tg docker/Dockerfile.tg-bot-api yoink/tg-bot-api:latest ;;
  backup)   just --justfile backend/justfile build-ops-image ;;
  all)
    build_runtime
    build_gateway
    run_build frontend docker/Dockerfile.frontend yoink/frontend:latest
    run_build nginx docker/Dockerfile.nginx yoink/nginx:latest
    ;;
  *) echo "Unknown target: $target" >&2; echo "Usage: $0 <yoink|gateway|frontend|nginx|tg|backup|all> [no_cache]" >&2; exit 2 ;;
esac

printf 'Build complete: %s\n' "$target"
