#!/usr/bin/env bash
# Remove every host-side build artifact (node_modules, pycache, .venv,
# dist). Hard rule from AGENTS.md: building and type-checking must happen
# inside containers; anything left on the host is a leak from a prior
# accidental `npm install` / `uv sync` on the host. This recipe makes
# 'reset to known-clean' a one-liner instead of a five-command ritual.

set -euo pipefail

find . -name "node_modules"  -not -path "*/.git/*" -prune       -exec rm -rf {} + 2>/dev/null || true
find . -type d -name __pycache__ -not -path "*/.git/*"          -exec rm -rf {} + 2>/dev/null || true
find . -name "*.pyc"         -not -path "*/.git/*" -delete 2>/dev/null || true
find . -name "*.egg-info" -type d -not -path "*/.git/*"         -exec rm -rf {} + 2>/dev/null || true
rm -rf .venv frontend/dist

echo "Host artifacts cleaned."
