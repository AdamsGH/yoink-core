# Local GitHub CI

`just ci-local` runs the workflow's browser and source verification jobs without publishing images or touching production data.

## Requirements

Use Linux amd64, a local Docker daemon with Buildx, and a configured Git commit identity. Keep at least 40 GiB free on project and Docker storage filesystems.

The containerd image store must use its standard `/var/lib/containerd` location.

## Commands

```sh
just ci-local
just ci-local browser
```

The default runs the workflow's `browser` and `verify` jobs as a pull-request event. The browser argument runs only the preflight.

The helper pins its controller tools and runner image. The workflow selects the Node version used by tests. See [local.mjs](../../scripts/ci/local.mjs) for exact pins.

## Source snapshot

The snapshot contains full repository histories, tracked edits, and explicitly selected helper files. It excludes live environment files, tokens, user configuration, and unrelated untracked files.

Dirty submodules, missing tracked files, and source symlinks stop the run. Commit submodule changes before checking a core snapshot.

Checkout is skipped inside act so local tracked edits remain in the snapshot.

## Isolation and cleanup

Run this helper only with trusted workflows. Its privileged controller uses a separate Docker daemon rather than the production Docker socket.

The helper mounts only its snapshot and uses bounded resources and bridge networks. Cleanup removes owned containers, volumes, images, caches, and temporary files after success, failure, or cancellation.

A bounded log tail remains under `backend/target/ci-local-logs/`. Cleanup failure is an error, not a successful run.

Remote image publication still requires the GitHub workflow's successful `required` job.
