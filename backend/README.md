# Rust backend foundation

Experimental replacement backend on `rewrite/rust-backend`. The Python backend and existing deployment remain unchanged. This workspace is not a production replacement yet.

## Run locally

Requires Rust/Cargo with edition 2024 support (declared MSRV 1.88), `just`, and Node.js 22+ for process smoke tests. Commit `Cargo.lock` to keep dependency resolution reproducible.

```sh
cd backend
just verify
just run
```

The default process listens on `127.0.0.1:8088`. It does not load `.env`, connect to PostgreSQL/Redis, launch Python, or contact Telegram.

```sh
curl http://127.0.0.1:8088/health/live
curl http://127.0.0.1:8088/health/ready
curl http://127.0.0.1:8088/api/v1/system
```

`/health/live` reports an active HTTP server. `/health/ready` reports readiness of the configured **diagnostic adapters only**, returning 503 before readiness or during draining. It does not certify downloader or other product functionality. `/api/v1/system` lists all five business modules as `planned`. These endpoints are unauthenticated and contain no user data. Keep the default loopback binding unless diagnostics exposure is intentional.

SIGINT and SIGTERM stop ingress, clear readiness, cancel adapters, and wait for the configured shutdown deadline. An unexpected adapter exit causes process failure and sibling cancellation. Exceeding the drain deadline aborts remaining owned tasks and exits unsuccessfully. There are no durable jobs to drain in this milestone.

## Configuration

All settings use the `YOINK_RUST_` prefix. Legacy settings such as `bot_token` are never read.

| Variable | Default | Meaning |
| --- | --- | --- |
| `YOINK_RUST_HTTP_BIND` | `127.0.0.1:8088` | Numeric IP address and port. Use port 0 for isolated fixtures. |
| `YOINK_RUST_TELEGRAM_ENABLED` | `false` | Exact `true` or `false`. |
| `YOINK_RUST_TELEGRAM_TOKEN` | unset | Required when Telegram is enabled. Use a separate test bot. |
| `YOINK_RUST_OWNER_ID` | unset | Required positive Telegram user ID when Telegram is enabled. |
| `YOINK_RUST_BOT_API_URL` | official Bot API | Optional HTTP(S) origin, such as `http://127.0.0.1:8082`. No credentials, path, query, or fragment. |
| `YOINK_RUST_TELEGRAM_POLL_TIMEOUT_SECS` | `30` | Long-poll duration, 1–50 seconds. Avoids short-poll request churn. |
| `YOINK_RUST_STARTUP_TIMEOUT_SECS` | `15` | Telegram initialization deadline, 1–300 seconds. |
| `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS` | `10` | Adapter drain deadline, 1–300 seconds. |

Telegram settings are inactive when Telegram is disabled. Supply enabled credentials through the process environment or your secret manager, not command-line arguments or committed files. Plain HTTP Bot API origins are intended only for trusted local/private transport. The URL option does not implement tdlight user-mode methods.

**Do not run this poller with the production bot token while Python is polling it.** Competing `getUpdates` consumers can interrupt production and consume its updates. The experimental adapter answers `/start`, `/help`, and `/status` only for the configured owner in a private chat, in English or Russian. It exposes no business commands. Polling errors fail the process rather than leave a falsely healthy adapter. The application does not delete webhooks or discard pending updates, so a test bot with an existing webhook must be configured for polling separately.

Logs use fixed safe error categories and only project-owned tracing targets. Raw Telegram errors and update contents must never be logged because request URLs contain bot tokens. This intentionally limits transport debugging until a redacted diagnostic facility exists.

## Dependency boundaries

| Crate | Responsibility |
| --- | --- |
| `yoink-domain` | Immutable validated download intent, operation IDs and lifecycle rules. No async runtime or transport framework. |
| `yoink-application` | Shared typed diagnostic service and honest module catalog. No framework dependency. |
| `yoink-http` | Axum routes invoking application services. |
| `yoink-telegram` | teloxide update handling and owner-only diagnostics. No environment reads. |
| `yoink-worker-protocol` | Versioned, bounded JSON messages for future isolated media/content executors. No process launching. |
| `yoink-server` | Configuration, resource ownership, startup, readiness, signal handling and supervision. |

Operation requests are not mutable conversation state. Cancellation intent is not executor acknowledgement. A retry preserves operation identity, increments its attempt fence, and leaves the previous terminal attempt unchanged. These are in-memory domain rules, not persistence or recovery guarantees.

Worker messages use a version, operation UUID and positive attempt number. Decode through the bounded frame APIs, not bare `serde_json::from_slice`, to enforce envelope validation. Each frame is at most 64 KiB. Source URLs can contain sensitive query parameters and are redacted in Debug output, but necessarily remain in the wire payload. Never log raw frames. Artifact paths are relative and lexically validated, but consumers must still enforce canonical workspace containment, symlink safety, byte limits, and cleanup ownership before opening files. SSRF/network policy and redirects require executor-level enforcement.

## Verification and limits

`just verify` checks formatting, denies Clippy warnings, runs workspace tests, builds with the lockfile, and runs process-level smoke tests. Smoke tests use ephemeral loopback listeners, fake Bot API credentials, and an isolated child environment. They cover HTTP diagnostics, SIGTERM, invalid configuration, occupied ports, and Telegram startup/shutdown without contacting Telegram.

Verified with Rust/Cargo 1.97.1. The declared MSRV has not been tested. The locked transitive dependency `proc-macro-error2` 2.0.1 currently emits a future-incompatibility warning about re-exporting `proc_macro` (Rust issue #127909). Current checks pass, but check the dependency before upgrading the compiler to a release that rejects it.

The existing repository's `just verify` remains the Python/frontend gate. All Python work remains inside containers. Do not run host Python, uv, pip, or pytest. The experimental workspace does not modify Python migrations or `uv.lock`.

Not implemented: persistent operations, effective grants/RBAC parity, frontend API compatibility, media execution, artifact handling, cache, worker transport, tdlight user mode, Inbox/Insight/Music/Stats scenarios, production deployment, or cutover. The shared application service currently serves diagnostics only. Domain and worker contracts are deliberately unconnected until a durable admission/execution slice is implemented.
