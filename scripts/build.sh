#!/usr/bin/env bash
# Build images with quiet success output and actionable failure logs.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

target="${1:-all}"
no_cache="${2:-}"
case "$no_cache" in 1|true|yes|on) cache=(--no-cache) ;; "") cache=() ;; *) echo "Invalid no_cache value: $no_cache (use 1/true/yes/on)" >&2; exit 2 ;; esac
cache_bust=$(git rev-parse HEAD)
common=(--build-arg CACHE_BUST="$cache_bust" "${cache[@]}")
log=$(mktemp)
trap 'rm -f "$log"' EXIT

run_build() {
  local name="$1" dockerfile="$2" image="$3"
  printf 'Building %-10s' "$name"
  if docker build "${common[@]}" -f "$dockerfile" -t "$image" . >"$log" 2>&1; then
    printf ' OK\n'
  else
    printf ' FAILED\n\n%s\n' "$(cat "$log")" >&2
    return 1
  fi
}

case "$target" in
  yoink)    run_build yoink docker/Dockerfile yoink/yoink:latest ;;
  frontend) run_build frontend docker/Dockerfile.frontend yoink/frontend:latest ;;
  nginx)    run_build nginx docker/Dockerfile.nginx yoink/nginx:latest ;;
  tg)       run_build tg docker/Dockerfile.tg-bot-api yoink/tg-bot-api:latest ;;
  backup)   run_build backup docker/Dockerfile.backup yoink/backup:latest ;;
  all)
    run_build yoink docker/Dockerfile yoink/yoink:latest
    run_build frontend docker/Dockerfile.frontend yoink/frontend:latest
    run_build nginx docker/Dockerfile.nginx yoink/nginx:latest
    ;;
  *) echo "Unknown target: $target" >&2; echo "Usage: $0 <yoink|frontend|nginx|tg|backup|all> [no_cache]" >&2; exit 2 ;;
esac

docker image prune -f >/dev/null 2>&1 || true
printf 'Build complete: %s\n' "$target"
