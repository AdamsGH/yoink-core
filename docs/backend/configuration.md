# Configuration

The Rust server reads process environment variables prefixed with `YOINK_RUST_`. It does not load `.env` and does not read the legacy Python lowercase names such as `database_url`, `bot_token`, or `owner_id`. Configure the Rust process explicitly; adding a Rust-prefixed setting does not configure the Python service.

## Server settings

All time values are integer seconds, default to the value shown, and must be in the range 1 through 300.

| Variable | Default | Behavior |
| --- | --- | --- |
| `YOINK_RUST_HTTP_BIND` | `127.0.0.1:8088` | HTTP diagnostics listener address. |
| `YOINK_RUST_STARTUP_TIMEOUT_SECS` | `15` | Total dependency startup deadline. |
| `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS` | `10` | Total graceful adapter drain deadline; remaining tasks are aborted on expiry. |

## Optional database

If `YOINK_RUST_DATABASE_URL` is absent, database connectivity and schema are omitted from readiness and database commands cannot run. If present, it must be a PostgreSQL URL with host and database path. Rust connects to the explicitly supplied URL; it does not discover the Compose database.

| Variable | Default | Behavior |
| --- | --- | --- |
| `YOINK_RUST_DATABASE_URL` | unset | Enables database startup/schema checks. Queue methods are library APIs, not HTTP endpoints. Treat as a secret. |
| `YOINK_RUST_DATABASE_MAX_CONNECTIONS` | `5` | Pool size, integer 1 through 32. |
| `YOINK_RUST_DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | Connection acquire and health-check timeout; 1 through 300. |
| `YOINK_RUST_DATABASE_HEALTH_INTERVAL_SECS` | `5` | Interval between health/schema checks; 1 through 300. |

A database health-check failure stops the adapters and exits unsuccessfully. Restarting the server is the responsibility of the process supervisor; the server does not restart itself.

## Optional Telegram adapter

Telegram is disabled unless explicitly enabled.

| Variable | Default | Behavior |
| --- | --- | --- |
| `YOINK_RUST_TELEGRAM_ENABLED` | `false` | Exact `true` or `false`; controls Telegram adapter startup. |
| `YOINK_RUST_TELEGRAM_TOKEN` | unset | Required when enabled; non-empty and contains no whitespace. |
| `YOINK_RUST_OWNER_ID` | unset | Required when enabled; positive signed-64-bit-range Telegram user ID. |
| `YOINK_RUST_BOT_API_URL` | unset | Optional HTTP(S) origin without credentials, path, query, or fragment. |
| `YOINK_RUST_TELEGRAM_POLL_TIMEOUT_SECS` | `30` | Poll timeout, 1 through 50 seconds. |

The adapter answers `/start`, `/help`, and `/status` only for the configured owner in a private chat, in English or Russian. It does not discard pending updates, delete webhooks, or implement tdlight user-mode methods. Configure a separate test bot for polling. Never share the production bot token with Python's poller.

The HTTP endpoints are unauthenticated diagnostics without user data. Keep the loopback bind unless exposing them is intentional. `/health/ready` returns 503 during startup and draining. A ready response does not certify product functionality.

Tokens and database URLs are redacted in configuration diagnostics. Keep them in the process environment or approved secret delivery mechanism, not in committed files or command output. Configuration alone does not enable authenticated business endpoints or media execution.
