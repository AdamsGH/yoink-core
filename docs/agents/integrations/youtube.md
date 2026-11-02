# YouTube authentication changes

- Keep OAuth credential resolution, refresh, and current-authority checks in Rust.
- Use the fixed Unix adapter and confined worker contracts. Do not restore the legacy standalone Express integration.
- Preserve versioned credential references. Replacement or deletion must invalidate stale admitted work.
- Do not forward personal credentials to PoToken helpers or unrelated origins.
- Keep helper ports private. Preserve supervisor drain, process-group cleanup, and readiness behavior.
- Read [cookie contracts](../../features/downloads/cookies.md) and [YouTube packaging](../../operations/youtube-services.md) before changing the boundary.
- Separate synthetic device/OAuth acceptance from verified live account authorization.
