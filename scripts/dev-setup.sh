#!/usr/bin/env bash
# One-time host setup for the Vite dev server.
#
# Why this script and not `npm install` directly:
#  - frontend/node_modules can be left root-owned by a prior container run
#    (build, shadcn, tsc); npm refuses to overwrite root-owned files from a
#    non-root user. We chown FIRST, then install.
#  - Plugin frontends share the same node_modules tree via symlink so
#    `import ... from '@dl/...'` resolves without re-installing per plugin.
#  - Stale .vite cache from a previous run on a different Vite version
#    breaks HMR; rm before install.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

sudo chown -R "$(id -u):$(id -g)" frontend/node_modules 2>/dev/null || true
rm -rf frontend/node_modules/.vite

( cd frontend && npm install )

for p in yoink-dl yoink-stats yoink-insight; do
    rm -rf "plugins/$p/frontend/node_modules"
    ln -sf "$(pwd)/frontend/node_modules" "plugins/$p/frontend/node_modules"
    echo "linked $p"
done

echo "Dev setup complete. Run: just dev"
