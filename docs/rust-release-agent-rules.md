# Rust builds and releases

- Production releases use `backend/VERSION` and immutable `yoink/rust-backend:MAJOR.MINOR.PATCH` tags. Never put raw image IDs, `latest`, dates or repair names in `yoink_rust_image`. Bump patch for fixes, minor for backward-compatible features, and major for breaking changes. Do not bump Cargo package versions for a bundle-only release.
- Use `backend/justfile` release recipes for builds, deployment and rollback. A newer candidate may coexist temporarily with an unconfirmed current release and its retained rollback anchor. Deploying it preserves the anchor and removes only the superseded runtime after readiness; recover interrupted deployment before proceeding. Docker health alone is not confirmation. Run `release-confirm` only after Adams confirms behavior; it removes the rollback image.
- Use the dedicated `yoink-core-release` builder, one runtime build and its bounded persistent cache. Do not purge this reusable cache after every task, export intermediate build images, or run global Docker prune commands. Run `release-trim` to enforce the 8 GiB cache budget. Historical application images are removed through `release-clean-images`, which protects active containers and unrelated repositories.
- Run backend Cargo checks through `backend/justfile`. Use the repository's dev/test profiles; enable symbols only for a specific debugging session, in a temporary target directory that you remove afterward.
- Give workers focused crate checks in their isolated worktrees. Use workspace/all-feature recipes for coordinator verification. Different feature graphs duplicate artifacts in a shared target. The coordinator owns shared Docker images. After integrating verified work, stop its host builds and remove its disposable host artifacts with `just clean-builds`; preserve source changes and reports.
- Use one target directory per active worktree. Reuse it across checks instead of creating per-call targets. Before a build, `check-build-space` must pass. If it fails, stop builds and clean inactive artifacts before retrying.

## Publication

Local deployment and remote publication are separate outcomes. `release-build` and `release-deploy` do not publish images or create a GitHub Release. Use the [publication contract](../README.md#quick-start) for image distribution and the [local release procedure](../backend/README.md#releases) for deployment.

1. Commit verified source and `backend/VERSION`. Push the release commit and its matching `vMAJOR.MINOR.PATCH` tag. Keep published tags on their original commits.
2. Track the tag-triggered `.github/workflows/ci.yml` run. Its `required` job must succeed before reporting image publication complete. The workflow and `scripts/container-images.mjs` own the image matrix and tag rules.
3. Publish the GitHub Release on that tag with concise English notes. Report local runtime readiness and remote publication separately. Retain the rollback anchor until Adams confirms behavior.
