#!/usr/bin/env bash
# Build one or all yoink docker images.
#
# Usage: scripts/build.sh <target> [no_cache]
#   target   one of: yoink | frontend | nginx | tg | backup | all
#   no_cache when set to "1" / "true" / "yes", passes --no-cache to every
#            docker build invocation. Use when buildkit's cache-key
#            heuristics keep an old layer alive after src/ edits (we
#            ran into this when `compose restart` was masking a stale
#            image; see justfile `restart` comment).
#
# CACHE_BUST=$(git rev-parse HEAD) is wired into Dockerfiles via an ARG so
# COPY src/ layers re-resolve whenever HEAD moves. Editing without
# committing still benefits from BuildKit content-hashing of src/, but if
# you suspect a stale layer pass no_cache=1.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

target="${1:-all}"
no_cache_arg=""
case "${2:-}" in
  1|true|yes|on) no_cache_arg="--no-cache" ;;
esac

cache_bust=$(git rev-parse HEAD)
common=(--progress=plain --build-arg CACHE_BUST="$cache_bust")
[ -n "$no_cache_arg" ] && common+=("$no_cache_arg")

build_yoink()    { docker build -f docker/Dockerfile           "${common[@]}" -t yoink/yoink:latest    .; }
build_frontend() { docker build -f docker/Dockerfile.frontend  "${common[@]}" -t yoink/frontend:latest .; }
build_nginx()    { docker build -f docker/Dockerfile.nginx     "${common[@]}" -t yoink/nginx:latest    .; }
build_tg()       { docker build -f docker/Dockerfile.tg-bot-api "${common[@]}" -t yoink/tg-bot-api:latest .; }
build_backup()   { docker build -f docker/Dockerfile.backup    "${common[@]}" -t yoink/backup:latest   .; }

case "$target" in
  yoink)    build_yoink ;;
  frontend) build_frontend ;;
  nginx)    build_nginx ;;
  tg)       build_tg ;;
  backup)   build_backup ;;
  all)      build_yoink; build_frontend; build_nginx ;;
  *)
    echo "Unknown target: $target" >&2
    echo "Usage: $0 <yoink|frontend|nginx|tg|backup|all> [no_cache]" >&2
    exit 1
    ;;
esac

docker image prune -f >/dev/null
