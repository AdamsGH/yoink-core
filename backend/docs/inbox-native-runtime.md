# Native Inbox composition

Inbox is opt-in: `YOINK_RUST_INBOX_ENABLED=true`, an explicitly migrated
`YOINK_RUST_DATABASE_PATH`, an existing private absolute `YOINK_RUST_INBOX_WORKSPACE`
(default `/tmp/yoink-inbox`), and the confined worker launcher/image are required.
No migration runs automatically. Inbox remains independent of DOWNLOADS_ENABLED
and TELEGRAM_ENABLED. Set `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=90` (or larger)
explicitly; the diagnostic-only default of 10 seconds intentionally cannot admit
Inbox's default budgets. The standard launcher `--check` is bounded by startup.

HTTP diagnostics listen before preparation. Routes are installed only with actual
Inbox dependencies and bearer auth enabled, and remain readiness-gated. Disabled
Inbox or absent bearer configuration leaves Inbox paths 404. All authorities share
the owning native database. HTTP re-resolves the authenticated actor's current
identity; background jobs use InboxAuthorization, not token claims. Telegram uses
the prepared, verified bot and identical identity/group policy, installs Inbox
commands before menus, and owns its notification future centrally.

All keys below have prefix `YOINK_RUST_`. Integer bounds are inclusive:

| Key | Default | Bounds |
|---|---:|---:|
| INBOX_JOB_TIMEOUT_SECS | 60 | 1-240 |
| INBOX_DRAIN_TIMEOUT_SECS | 5 | 1-30 |
| INBOX_LEASE_SECS | 120 | 1-3600 |
| INBOX_EXECUTOR_TIMEOUT_SECS | 20 | 1-60 |
| INBOX_SOCKET_TIMEOUT_SECS | 15 | 1-60 |
| INBOX_COMPLETION_TIMEOUT_SECS | 30 | 1-120 |
| INBOX_GITHUB_TIMEOUT_SECS | 15 | 1-60 |
| INBOX_TEXT_TIMEOUT_SECS | 5 | 1-60 |
| INBOX_INTERVAL_MS | 1000 | 1-60000 |
| INBOX_BATCH | 1 | 1-16 |
| INBOX_MAX_ATTEMPTS | 3 | 1-10 |
| INBOX_CONTENT_MAX_CHARS | 40000 | 100-128000 |
| INBOX_NOTIFICATION_LEASE_SECS | 120 | 1-3600 |
| INBOX_RECOVERY_BATCH | 64 | 1-256 |
| INBOX_DISPATCH_BATCH | 1 | 1-16 |

`INBOX_INTERVAL_MS` controls both job polling and Telegram notification polling.

`INBOX_GITHUB_ENDPOINT` optionally selects an operator-trusted HTTP(S) gateway;
never populate it from user input. Default origins are real GitHub API/raw.
Redirects remain disabled. `INSIGHT_GATEWAY_BASE`, `INSIGHT_GATEWAY_KEY`,
`INSIGHT_OPENROUTER_KEY`, and `INSIGHT_MODEL` configure the real Insight completion
service; user credential/language/current-route lookup stays inside Insight.
Parent configuration debug output redacts all Inbox configuration. Content carries
no cookies, credentials, private-origin exemptions or media dependencies.

Executor artifacts/output/workspace have fixed caps of 1 MiB/1 MiB/2 MiB and one
artifact; content output does not admit artifacts. Socket <= executor <= job,
completion and GitHub <= job. Job + drain + four DB acquisition budgets must be
strictly below both lease and shutdown. Notification lease exceeds text request;
dispatch batch times text deadline plus drain fits shutdown.

## Ownership and cancellation

One centrally supervised future sequentially claims enrichment, classification,
and stars-sync jobs, including expired leases recovered by the store's claim API.
Each claim retains its fencing token through execution and bounded recovery.
A job-specific token reaches concrete ContentStages, Insight's actual Inbox
adapter macro, and cancellable real GitHub reads. Whole-job deadline and owning
shutdown both cancel these adapters and drain execution before fenced recovery.
Only owning shutdown records `cancelled` under the live lease, restoring its
consumed attempt. A whole-job deadline instead records the existing external
failure/error semantics: it consumes an attempt, retries only below
`INBOX_MAX_ATTEMPTS`, and becomes terminal at that bound. Independently cancelled
adapters also count as failures rather than shutdown interruptions. A completed
job's stale failure write is a fenced no-op. GitHub mutation cancellation remains
uncertain, never cancellation proof or automatic retry.

No owned loop is detached. JoinSet failure clears readiness and cancels siblings;
shutdown drains before closing the database. Notification cancellation lets an
admitted bounded text attempt finish; unknown send/edit ledger effects remain
unknown, not retryable. Forced server abort is still a fatal shutdown failure,
not evidence that an external effect did not occur. Database/invariant failure in
recovery is actionable; it cannot promise successful lease release if storage is
unavailable. Executor cancellation uses the public kill/reap implementation;
forced dropped-future cleanup remains its documented last-resort contract.

## Checks / coordinator gates

Use existing recipes with `CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=target/inbox-server`:
`just check-fixture-server`, `just test-fixture-server`, and
`just test-crate yoink-http`. Recipes preflight space; existing zero-debug profiles
apply. Tests cover bounded/redacted config,
late real HTTP installation/current bans, disabled/no-bearer paths, real GitHub
read cancellation, and an admitted stars-sync job shutdown restoring its attempt
and releasing its lease on a migrated disposable database. Repeated whole-job
deadlines are tested through the configured attempt limit: terminal failure has
no remaining lease or further dispatch, and subsequent eligible work completes.

Coordinator still owns full workspace, image rebuilds (including pinned content
libraries), import, actual-process cancellation/restart/child reaping, Telegram
save/completion provenance and unknown-effect gates, and SPA acceptance. These
focused tests do not substitute for those real-process checks.
