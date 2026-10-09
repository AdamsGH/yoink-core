#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
mkdir -p .pi
exec flock -n -E 75 .pi/native-maintenance.lock node scripts/native-backup.mjs "$@"
