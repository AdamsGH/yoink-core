# Download history and automatic proxy routing

The native backend serves `/api/v1/dl/downloads`, `/api/v1/dl/downloads/domains`, and `/api/v1/dl/downloads/{id}/retry`. History combines imported `download_log` rows with terminal native operations. Imported IDs remain numeric; native IDs are stable operation UUIDs. Native rows expose available operation metadata; unavailable titles, duration, and file sizes remain null. Reads are scoped to the authenticated actor and recheck current authorization. Retries use current preferences and normal admission checks, and deliver only to the authenticated actor's private chat.

History supports `offset`, `limit`, `domain`, `search`, `status`, `date_from`, and `media_type`. Dates use UTC and responses use RFC3339 timestamps. Filtering applies before counting and pagination.

`YOINK_RUST_DOWNLOAD_PROXY_DOMAINS_JSON` controls automatic use of the configured `YOINK_RUST_DOWNLOAD_PROXY_URL`. Defaults cover TikTok, e621, X, and Twitter, including subdomains. This policy is independent of the personal proxy toggle. An enabled personal proxy takes precedence; a disabled personal proxy is never used by automatic routing. Frozen routes are revalidated before execution and never silently replaced with direct access.

Verification uses `just -f backend/justfile test-download-repair`, `just verify-fe`, and `just -f backend/justfile test-download-history-spa`. The SPA check uses a disposable native database and disables the local development token in its production frontend build.

Run CPU-intensive checks sequentially on the deployment host. `just -f backend/justfile build-media-limited` uses Buildx per-step resource limits: one CPU, 3 GiB of memory, and no swap. It builds targets sequentially so parallel stages cannot multiply those limits. The installed BuildKit must support `--resource`; unsupported versions fail without starting an unconstrained build. Native checks can run under a user systemd unit with `CPUQuota=100%`, `MemoryMax=3G`, and `MemorySwapMax=0`. Cargo and the container build default to one compilation job.
