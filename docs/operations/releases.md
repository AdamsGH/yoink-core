# Local releases

The release manager builds and deploys immutable backend images. It stores private state under `backend/.release`. It does not publish images or create GitHub Releases.

## Version and candidate

`backend/VERSION` identifies the deployable bundle, including Rust and confined workers. Cargo package versions are separate.

Use patch for fixes, minor for compatible features, and major for breaking changes. Published version tags cannot move to another source commit.

```sh
just --justfile backend/justfile release-status
just --justfile backend/justfile release-version patch
just --justfile backend/justfile release-build
```

A source build uses a verified snapshot. Later working-tree edits cannot change an in-progress build. The image supplies the version reported by `/api/v1/system`.

The first build requires `.env` and no existing `yoink` container. It initializes release state before updating the runtime pin. Compilation failure leaves both unchanged.

If status reports `phase=bootstrapping`, repeat `release-build` to recover initialization.

## Deploy and accept

```sh
just --justfile backend/justfile release-deploy
```

Deployment changes only `yoink_rust_image` in `.env`, recreates only the backend, and checks readiness. Failed startup restores the immediately preceding runtime.

Deployment does not migrate the database. If the candidate needs a schema upgrade, follow the offline migration and recovery procedure before deployment.

Check real behavior before confirmation. Readiness and Docker health are not human acceptance.

```sh
just --justfile backend/justfile release-confirm
```

Confirmation removes the retained rollback image. Do not run it until the operator accepts the release.

An unconfirmed release does not block a newer fix. Deploying a newer candidate preserves the existing rollback anchor. Successful deployment then removes only the superseded unconfirmed runtime.

## Recovery

- `release-rollback` restores the retained previous image without rebuilding.
- `release-discard` removes an unwanted candidate before deployment.
- `release-status` reports interrupted transitions and retained references.

If status reports `phase=retiring`, repeat `release-deploy` or use `release-rollback`. Resolve that transition before another build or confirmation.

Image rollback does not restore data or downgrade the schema. Check compatibility before selecting an older binary.

## Prebuilt upgrades

Check out the release tag. Run `just pull-images` at the repository root. Run `just build yoink` to validate and register the downloaded runtime.

This registration does not compile Rust, change the active pin, or restart the service. Source and version metadata must match the checkout.

Auxiliary services need separate image updates and recreation. Backend rollback does not restore them.

## Adopt an existing installation

From `backend/`, use `just release-init CURRENT_IMAGE PREVIOUS_IMAGE PREVIOUS_VERSION` to adopt existing images. It does not rebuild or restart them.

The one-time `0.1.0-legacy.1` predecessor represents an unversioned rollback snapshot. It is not a historical release claim.

## Build resources and reports

The dedicated release builder uses one CPU, 3 GiB RAM, and serialized build steps. Its cache target is 8 GiB with a 20 GiB free-space target. These are garbage-collection targets, not active-build filesystem quotas.

The wrapper trims the cache after success or failure and stops the builder. `release-trim` performs the same maintenance. The reusable cache is intentional.

Build reports remain under `backend/target/media-reports/releases/`. Each version has a log, JSON summary, and cache inventories. The summary records wall time and step durations. Concurrent step durations are not additive.

`release-clean-images` removes only eligible Yoink aliases. It protects active containers and unrelated repositories. Never use global Docker pruning as release cleanup.
