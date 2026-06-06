#!/usr/bin/env bash
# Cookie-extraction browser (Kasm). Behind the `cookies` compose profile so
# it doesn't sit in the regular `just up` set: this is a one-shot tool you
# spin up, log into a site, dump cookies, then tear down.
#
# Usage: scripts/browser.sh <action>
#   up    start container, print VNC URL
#   down  stop
#   logs  tail logs

set -euo pipefail

action="${1:-up}"

case "$action" in
  up)
    docker compose --profile cookies up -d yoink-browser
    echo "Browser: http://$(hostname):6902"
    ;;
  down)
    docker compose --profile cookies down yoink-browser
    ;;
  logs)
    docker compose --profile cookies logs -f yoink-browser
    ;;
  *)
    echo "Usage: $0 [up|down|logs]" >&2
    exit 1
    ;;
esac
