# Rust backend

Native Rust bot/API with shared Turso 0.7.2 storage and integrated Download, Music, Stats, Insight and Inbox products. Media/content execution uses the container-only confined Python worker. Shared authorization, durable delivery journals, bounded imports, search ownership and cancellation-aware startup/shutdown are verified by library and actual-process acceptance. Production Compose selects this runtime; standalone defaults remain offline. See [deployment](../docs/backend/deployment.md) for source migration, exclusive polling, configuration, backups and rollback.

## Releases

On a new installation, `just build all` from the repository root builds and initializes the first runtime without starting it. It requires `.env` but no previous image or private release state. Compilation failures leave both `.env` and release state unchanged.

Initialization persists recovery state before updating `.env`. Repeat `release-build` if status reports `phase=bootstrapping`. Initialization refuses any existing `yoink` container. Adopt an existing healthy installation with `release-init` instead. For prebuilt releases, follow the root [quick start](../README.md#quick-start).

`VERSION` is the SemVer version of the deployable bundle, including Rust and Python workers. Cargo package versions are independent. Image tags are immutable. Version changes affect image metadata, not Rust compilation inputs. The image supplies `YOINK_RUST_RELEASE_VERSION` to the typed runtime configuration, so `/api/v1/system` reports the release version without recompilation. Standalone Cargo runs fall back to the package version.

```sh
cd backend
just release-status
just release-version patch # Or minor / major.
just release-build
just release-deploy
# Check real behavior. Docker health does not confirm the release.
just release-confirm
```

`release-deploy` atomically updates only `yoink_rust_image` in `.env`, recreates only `yoink`, and checks readiness. Failed startup restores the runtime active immediately before deployment. `release-rollback` restores the retained previous version without rebuilding. Confirmation deletes the previous image, but is not required to build a newer fix. Before deployment, `release-discard` can remove an unwanted candidate. Internal image IDs remain in the ignored `.release/state.json` for tag integrity checks, not in `.env`.

For a prebuilt upgrade, check out its published Git release tag and run `just pull-images` from the repository root. Then run `just build yoink` to register the downloaded runtime as a candidate. Source and version labels must match the checkout. This does not compile Rust, change the active runtime pin, or restart a container. Use `release-deploy` and the normal confirmation procedure only when ready for cutover.

Auxiliary services need separate recreation and compatible image pins. Runtime rollback does not restore their images.

While a release awaits human confirmation, a newer candidate can temporarily coexist with the current release and its retained rollback anchor. Deploying that candidate preserves the anchor and leaves `confirmed=false`. Only after readiness succeeds does deployment remove the superseded, unconfirmed runtime. There is at most one candidate and, after successful deployment, only the current release and one rollback version. No synthetic confirmation or anchor deletion is needed to ship a fix.

Interrupted deployment retains all recovery references. If status shows `phase=retiring`, repeat `release-deploy` to recheck readiness and finish scoped image cleanup, or run `release-rollback`. Recovery restores the immediate pre-deployment runtime if its image remains; after that image was removed, rollback uses the retained anchor. If the intended runtime is already healthy and uses the exact retained image, recovery restores its pin without recreating the container. An in-use image is never removed. Resolve interrupted deployment before building or confirming another release.

Discarded versions cannot be reused. Release state retains the highest built version even after rollback. Builds use a verified source snapshot, so later working-tree edits cannot change an in-flight release. Image rollback does not restore database contents or downgrade schemas. Check schema compatibility before deployment. Incompatible schema changes require an explicit migration and recovery decision.

The dedicated `yoink-core-release` builder uses 1 CPU, 3 GiB RAM, serialized build steps, and automatic garbage collection with an 8 GiB cache budget and 20 GiB free-space target. These are garbage-collection targets, not a hard filesystem quota during an active build. The release wrapper trims the cache after successful or failed builds, then stops the builder to release RAM. `release-trim` does the same on demand. This persistent cache is intentional. Do not delete it after each release. Host-side Cargo checks still use the separate build-resource rules.

`release-build` saves its output to `target/media-reports/releases/VERSION.log`. It prints the log path before compilation, so `tail -f` can follow progress. The adjacent JSON report records wall time, per-step durations, cache hits, compiled crates and failure status. Cache inventories before the build and after trimming use the `.cache-before.jsonl` and `.cache-after.jsonl` suffixes. These reports remain after failures and cancellation. Step durations can overlap, so use wall time for comparisons.

Rust and Python base images use fixed digests. Update them explicitly for security and dependency updates instead of changing build inputs through mutable tags.

`just test-release` checks release transitions, failure recovery and report parsing. `just test-release-config` checks native runtime configuration in the bounded builder without exporting a compiler image. `just test-release-build` checks unchanged/version-only/worker-only cache reuse and focused launcher/Telegram rebuilds. Each build uses a new source snapshot and a full builder bootstrap, trim and stop cycle. The test rejects third-party dependency recompilation and worker dependency installation for small Rust changes. It also runs container worker tests, including the HTTP release version check. Temporary contexts and images are removed on failure or success. The reusable builder cache, logs, build reports, cache inventories and full-cycle timings under `target/media-reports/release-build` remain.

For an existing unversioned installation, `just release-init CURRENT_IMAGE PREVIOUS_IMAGE PREVIOUS_VERSION` adopts images without rebuilding or restarting production. The one-time `0.1.0-legacy.1` predecessor denotes an unversioned rollback snapshot, not a claimed historical release. `release-clean-images` removes obsolete Yoink runtime/fixture aliases while retaining the current test fixture and the `experimental` compatibility alias pointing at the current runtime. It refuses to remove an image still needed by another container. `release-clean-legacy-cache` removes only identified old Yoink cache records from the default builder, not shared parents, mutable cache mounts or other projects.

## Development

Read the [backend documentation](../docs/backend/index.md) for architecture, configuration, persistence, access control, and development workflows. The [native storage decision](../docs/decisions/0003-native-turso-runtime.md) explains single-process file ownership, migration checks and the changed security boundary.

```sh
cd backend
just verify
just run
```

Defaults are offline: HTTP listens on `127.0.0.1:8088`, Telegram and database access are disabled, and `.env` is not loaded.

```sh
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/api/v1/system
```

For the real media path and its delivery-uncertainty rules, see [the media slice guide](../docs/backend/media-slice.md). The [downloader compatibility matrix](../docs/backend/downloads-parity.md) separates accessible commands, engine capabilities and remaining legacy behavior. `just verify-media` builds separate runtime and test-controller images, then tests the Python worker and full download-to-upload path against a disposable native database volume and a fixture Bot API.

`just test-media presentation` checks linked-title captions, cache parity, native upload activity and cancellation, success-only private message cleanup, interactive cut formatting and group routing against isolated fixtures. `just test-media delivery` also checks restart-safe cleanup and retention of messages after failed, partial or uncertain delivery. `just test-live-youtube` checks the real public download, uploaded media, caption and cleanup without the production Telegram token.

For native database tests, see [development prerequisites and verification](../docs/backend/development.md). `just test-db` uses private temporary files and the real server binary without Docker or a PostgreSQL server. Media tests and all Python work remain inside containers. The separate [PostgreSQL importer](../docs/backend/import.md) is verified against disposable Alembic fixtures, including a real TLS import through its pinned-compiler container. The [backup/restore utility](../docs/backend/backup-restore.md) is verified on isolated data, including container disk exhaustion and process crashes. Source migration requires a stopped-writer import and equivalence verification before activation; isolated candidates are not final cutover evidence.

The [Stats search component](../docs/backend/stats-search.md) uses a separate Tantivy index with durable native change tracking. Its approved tokenizer contract and explicit PostgreSQL differences are covered by a shared corpus. Stats commands, analytics and import routes are integrated into the shared runtime.

The standalone [Turso evaluation](experiments/turso/README.md) and [decision 0002](../docs/decisions/0002-turso-evaluation.md) retain the original feasibility checks. The Rust runtime now uses native Turso. The legacy PostgreSQL database remains a historical recovery snapshot, not a current writable copy of native data.

Never start another poller with the production token. Stop the legacy poller before activating native; competing `getUpdates` consumers can consume updates or interrupt service.

For native AI generation and Telegram delivery investigation, see [Insight diagnostics](../docs/backend/insight-diagnostics.md). `just insight-diagnostics` produces a bounded metadata-only report; inactive restores can establish historical delivery state without querying the active database.

### Personal credential controls

When a system proxy is configured, native downloads automatically proxy YouTube (including `youtu.be`), TikTok, e621, X and Twitter. This routing is independent of the personal proxy toggle. `YOINK_RUST_DOWNLOAD_PROXY_DOMAINS_JSON` replaces that list when set. Keep YouTube in the deployed list on networks where direct TLS connections fail. `just test-download-proxy` checks ordinary admission with the personal toggle disabled. `just test-live-youtube` uses the effective production proxy settings with an isolated native database, a fixture Bot API and a real confined media download. It never uses the production Telegram token or uploads to a real chat.

Approved, unbanned users can manage only their own non-pool cookies and toggle
only their selected proxy in their own positive private, non-threaded chat.
Operational proxy URL editing and shared cookie pool administration are unchanged.
Personal cookie writes have finite configurable bounds:
`YOINK_RUST_PERSONAL_COOKIE_DOMAINS` (128),
`YOINK_RUST_PERSONAL_COOKIE_ACTOR_BYTES` (67108864, 128 × 512 KiB), and
`YOINK_RUST_PERSONAL_COOKIE_GLOBAL_BYTES` (1073741824, conservative 1 GiB).
These count UTF-8 content bytes, exclude pool rows, and are checked under the
same immediate transaction as canonical-domain replacement and replay watermark.
Domain capacity conservatively counts physical rows (including malformed imports).
Lowering limits never removes imported data or blocks reads/deletes/clear or
non-growing replacements. Rejected growth consumes its message watermark so a
replay cannot install credentials after capacity is freed. No schema change is needed.
`/cookie token` issues a single-use browser-extension sync token. Configuring
`YOINK_RUST_COOKIE_WEB_ORIGIN` adds a WebApp link to the existing `/cookies` page.
No mobile `/cookie-sync` link is emitted because that page is not implemented.
The operator must serve the frontend and API at the configured HTTPS origin.
See [the cookie web contract](docs/cookie-web.md) for the API and verification
boundaries; this is not complete cookie/downloader parity.

Coordinator-only opt-in native acceptance: `node scripts/test-media.mjs personal-controls`
against already built fixture/backend images. This scenario is deliberately not part
of `all`: it exercises a nonowner actual cookie file upload, URL import, owned
cookie extraction, selected proxy on/off routing, callback context denials,
role/ban revocation, immutable replay and secret-free replies on disposable state.
