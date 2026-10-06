# Core configuration compatibility handoff

## Implemented contracts

- `GET/PATCH /api/v1/bot-settings`: existing authorization and persistence retained. The HTTP boundary now passes a typed `BotSettingsPatch` to identity. Storage IDs accept both JSON integers and decimal strings used by the frontend. Invalid/unknown settings return 422 with a field-specific `detail`; a mixed invalid patch writes nothing. Null keeps the Python `exclude_none=True` no-op semantics. Unset storage IDs read as null rather than empty strings. Other imported rows are preserved, not replaced by PATCH.
- `GET /api/v1/_meta/openapi.json`: authenticated current admin/owner, no-store. Valid OpenAPI 3.1 documentation of the core configuration slice (not a complete product/admin API schema).
- `GET /api/v1/threads/status`: current admin/owner, `{ "available": false }`. The native runtime has no user-mode session adapter; bot-token availability is not substituted for it.
- `POST /api/v1/threads/scan/{group_id}`: current owner; malformed/nonnegative ID 422; supported IDs 503 with actionable native-adapter diagnostic. No fake successful scan or mutation.
- `GET /health`: public, HTTP 200 even when degraded, `status` and `checks.db/bot/disk` as in Python. Actual native `SELECT 1` latency probe, cached bot state, read-only disk statistics from the configured native database parent directory (`Database::path()`), executed through `spawn_blocking`; task/probe failures are sanitized. Errors are sanitized. `/health/ready` remains the separate 503 readiness gate.

## Exact source evidence

- `frontend/src/lib/api/bot-settings.ts`: GET, single-key PATCH, PATCH-many, tag maps/features.
- `frontend/src/pages/admin/bot-settings/index.tsx:saveStorage`: sends storage ID **strings** or null, not numeric JSON.
- `src/yoink/core/api/routers/bot_settings.py`: admin/owner read; owner update; partial update excluding None; string row values.
- `src/yoink/core/db/repos/bot_settings.py:DEFAULTS`: null storage defaults, open access, owner cookies role, empty tag map.
- `frontend/src/lib/api/meta.ts` and `src/yoink/core/api/app.py`: admin metadata route under `/api/v1`.
- `frontend/src/lib/api/threads.ts` and `src/yoink/core/api/routers/threads.py`: user-session availability and owner-only scan.
- `src/yoink/core/api/health.py` and `src/tests/test_health.py`: public 200, degraded body, db latency and disk checks. `docker-compose.yaml` and `docker/nginx/nginx.conf.template` request `/health`.
- Existing `/settings` language/theme read/write is already implemented in HTTP and identity, including typed validation and current bans. It was not rewritten.

## Authorization and regression evidence

Immediate configuration write transactions still call `admin_auth` against current rows/ban time before any write. Typed configuration does not trust JWT role claims. Tests cover frontend strings, defaults, atomic rejection, null no-op, owner restriction, a previously minted owner's token denied after a database ban, metadata authorization, unavailable scan, real health probe, and close/reopen persistence including unrelated imported rows. Health repair tests additionally verify real disk success for a DB outside `/app/data`, configured parent-directory access before/after reopen, no-DB diagnostics with public 200 and readiness 503, and sanitized blocking-task failure/missing-path diagnostics.

Focused verification (single local `backend/target`, jobs=1, resource preflight via justfile):

- `just test-crate yoink-http`: exit 0, 35 tests passed (16 unit, 15 auth/config HTTP, 3 groups, 1 NSFW), 0 failed.
- `just test-crate yoink-identity`: exit 0, 52 tests passed, 0 failed, including 3 typed parser unit tests and one configuration disk reopen test.
- `cargo fmt -p yoink-http -p yoink-identity -- --check`: exit 0.
- `git diff --check`: exit 0.
- Final repair `just check-build-space`: exit 0, target 4.04 GiB, free 203.8 GiB.
- Initial cold builds exceeded the tool's 120-second call limit; the completed reruns above are the verification evidence.
- No workspace/image/Python checks, external services, commits, or production mutation performed.

## Coordinator integration and limitations

Owned modules: HTTP `configuration.rs`, `configuration-openapi.json`, `health.rs`; identity `admin/configuration.rs`; configuration tests and fixture SQL. Shared hunks: HTTP `lib.rs` module/health router composition, `auth.rs` configuration merge, `admin.rs` typed PATCH/null read handling; identity `admin.rs` module, authorization/probe and typed entrypoint methods. HTTP manifest/lock add the already pinned rustix for read-only disk statistics (no version upgrades).

Keep the coordinator's temporary `.cargo/config.toml` jobs=1 while builds are active; **exclude it from integration**. No server port, Docker or nginx changes: coordinator owns cutover wiring. No schema/model changes or migration required.

Remaining blockers: native user-session topic scanning and Python `/forum/history`, `/forum/search`, `/messages` proxy APIs require a Telegram user-session adapter; there is no typed core frontend history/log/lifecycle client. `/metrics` exists in Python but has no typed frontend client and native lacks the legacy counters/histograms collector. Metadata intentionally only documents configuration; full API schema aggregation is still integration work. No shell/host-command/restart route was introduced. Product and downloader settings/cookie APIs and plugin source copies are untouched.
