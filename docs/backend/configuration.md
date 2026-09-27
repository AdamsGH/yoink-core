# Configuration

The Rust server reads process environment variables prefixed with `YOINK_RUST_`. It does not load `.env` and does not read legacy Python lowercase names such as `database_url`, `bot_token`, or `owner_id`. Configure the Rust process explicitly; adding a Rust-prefixed setting does not configure the Python service. Defaults are offline: Telegram, PostgreSQL, and downloads are disabled, HTTP binds to loopback, and no production bot credential is required.

## Server settings

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_HTTP_BIND` | `127.0.0.1:8088` | Socket address | HTTP diagnostics listener. |
| `YOINK_RUST_STARTUP_TIMEOUT_SECS` | `15` | 1 through 300 seconds | Total dependency startup deadline. |
| `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS` | `10` when downloads are disabled; `60` when enabled | 1 through 300 seconds | Graceful adapter and database drain deadline; remaining tasks are aborted on expiry. When downloads are enabled, it must exceed the delivery timeout plus heartbeat interval. |

## Optional database

If `YOINK_RUST_DATABASE_URL` is absent, database connectivity and schema are omitted from readiness. If supplied, it must use the `postgres` or `postgresql` scheme and include a host and non-empty database path, with no fragment or whitespace. Rust connects only to this explicitly supplied URL; it does not discover the Compose database.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_DATABASE_URL` | unset | PostgreSQL URL | Enables database startup/schema checks. Treat as a secret. |
| `YOINK_RUST_DATABASE_MAX_CONNECTIONS` | `5` | 1 through 32 | Pool size. |
| `YOINK_RUST_DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | 1 through 300 seconds | Connection acquire and health-check timeout. |
| `YOINK_RUST_DATABASE_HEALTH_INTERVAL_SECS` | `5` | 1 through 300 seconds | Interval between health/schema checks. |

A database health-check failure stops the adapters and exits unsuccessfully. Restarting the server is the process supervisor's responsibility.

## Optional Telegram adapter

Telegram is disabled unless explicitly enabled.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_TELEGRAM_ENABLED` | `false` | Exact `true` or `false` | Controls Telegram adapter startup. |
| `YOINK_RUST_TELEGRAM_TOKEN` | unset | Required when enabled; non-empty, no whitespace | Bot token. Do not share a live Python bot token with the Rust poller. |
| `YOINK_RUST_OWNER_ID` | unset | Required when enabled; positive ID up to signed 64-bit maximum | Configured owner identity. |
| `YOINK_RUST_BOT_API_URL` | unset | HTTP(S) origin without credentials, path, query, or fragment | Optional Bot API endpoint override. |
| `YOINK_RUST_TELEGRAM_POLL_TIMEOUT_SECS` | `30` | 1 through 50 seconds | Long-poll timeout. |

The adapter's outer command gate remains restricted to the configured owner in a private chat. Existing `/start`, `/help`, and `/status` diagnostics remain available. With downloads enabled, the same gated bot additionally accepts `/download <http(s) URL>`, `/job <UUID>`, and `/cancel <UUID>`. The media service independently loads the read-only identity snapshot and requires an unblocked `user`-or-higher private-scope identity at admission and before work and delivery. This does not open the bot to all users.

## Optional media downloads

Downloads are disabled unless explicitly enabled. Enabling them requires both explicit Telegram configuration and a PostgreSQL URL. Runtime also requires the experimental container marker and fixed Python worker files, so enabling media from an ordinary host process fails startup. The worker command is fixed to `/usr/local/bin/yoink-worker-launcher`, which enforces Landlock isolation before executing `/usr/local/bin/python -I -B /opt/yoink-worker/main.py`. Startup checks launcher confinement support. Attempt directories are owner-only (`0700`). It does not invoke host Python. Defaults and accepted values are:

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_DOWNLOADS_ENABLED` | `false` | Exact `true` or `false` | Enables media service; requires Telegram and PostgreSQL configuration and the experimental container image. |
| `YOINK_RUST_DOWNLOAD_CONCURRENCY` | `2` | 1 through 8 | Concurrent download workers. |
| `YOINK_RUST_DOWNLOAD_LEASE_SECS` | `60` | 3 through 300 seconds | PostgreSQL worker lease. Must be at least three heartbeat intervals and at least the delivery timeout. |
| `YOINK_RUST_DOWNLOAD_HEARTBEAT_SECS` | `5` | 1 through 100 seconds | Lease heartbeat interval. |
| `YOINK_RUST_DOWNLOAD_POLL_INTERVAL_MS` | `250` | 10 through 10000 milliseconds | Queue poll interval when no work is claimable. |
| `YOINK_RUST_DELIVERY_TIMEOUT_SECS` | `30` | 1 through 300 seconds | Maximum time waiting for Telegram delivery; cannot exceed the lease. |
| `YOINK_RUST_DOWNLOAD_MAX_BYTES` | `20971520` | 1 through 52428800 bytes | Per-artifact byte limit. |
| `YOINK_RUST_DOWNLOAD_WORKSPACE_MAX_BYTES` | `104857600` | At least artifact limit, at most 209715200 bytes | Maximum workspace bytes per attempt. |
| `YOINK_RUST_DOWNLOAD_WORKSPACE` | `/tmp/yoink-work` | Absolute dedicated directory, not filesystem root | Per-attempt workspace root. |
| `YOINK_RUST_DOWNLOAD_TIMEOUT_SECS` | `120` | 1 through 3600 seconds | Worker process deadline. |
| `YOINK_RUST_WORKER_SOCKET_TIMEOUT_SECS` | `15` | 1 through 300 seconds | Worker network socket timeout. |
| `YOINK_RUST_WORKER_PROCESSING_TIMEOUT_SECS` | `min(DOWNLOAD_TIMEOUT_SECS, 300)` | 1 through 3600 seconds, at most the worker deadline | Per-processing-subprocess deadline, including ffprobe. |
| `YOINK_RUST_DOWNLOAD_MAX_ITEMS` | `20` | 1 through 100 | Maximum returned artifacts per attempt. |
| `YOINK_RUST_WORKER_MAX_OUTPUT_BYTES` | `1048576` | 65536 through 4194304 bytes | Maximum worker stdout event bytes. |
| `YOINK_RUST_WORKER_ALLOWED_PRIVATE_ORIGINS` | `[]` | JSON array of at most 16 HTTP(S) origins | Narrow exception to worker private-address blocking, for trusted isolated test fixtures only. Entries may contain scheme, host, and optional port, but no credentials, path, query, fragment, or whitespace. Do not use this for production destinations. |
| `YOINK_RUST_WORKER_ALLOWED_PROXY_ORIGINS` | `[]` | JSON array of at most 16 HTTP(S) origins | Allowed proxy endpoints without credentials. Does not select a proxy or grant access to cookies. Request credentials travel separately in the stdin protocol. |
| `YOINK_RUST_DOWNLOAD_PROXY_URL` | unset | HTTP(S) proxy URL, optionally with credentials | System fallback only when the user's stored proxy setting is enabled and no personal proxy is set. Treat as a secret. Its origin must also be explicitly allowed by `WORKER_ALLOWED_PROXY_ORIGINS`. Missing or invalid enabled proxy fails closed, never switches to direct access. |
| `YOINK_RUST_DOWNLOAD_NO_COOKIE_DOMAINS_JSON` | `[]` | JSON array of at most 64 hostnames or IP addresses | Suppresses cookie selection for normalized domains and their subdomains. Entries cannot contain URL paths, credentials, ports, or whitespace. Does not bypass unsupported YouTube OAuth checks. |
| `YOINK_RUST_DOWNLOAD_MAX_ATTEMPTS` | `3` | 1 through 10 | Queue retry budget. A definitive invalid cached file ID before any confirmed delivery grants at most one compensating attempt and permanently bypasses cache for that operation. |
| `YOINK_RUST_DOWNLOAD_RETRY_DELAY_SECS` | `5` | 0 through 300 seconds | Delay before retryable failures. |
| `YOINK_RUST_DOWNLOAD_CACHE_TTL_SECS` | `2592000` | 0 through 31536000 seconds | Lifetime of complete confirmed Telegram file-ID batches, partitioned by actor, bot, request parameters and credential versions. Zero disables reads and publication. Hits do not refresh expiry. Live authorization and current artifact limits still apply. |
| `YOINK_RUST_DOWNLOAD_MAX_PENDING` | `100` | 1 through 10000 | Maximum pending operations globally. |
| `YOINK_RUST_DOWNLOAD_MAX_PENDING_PER_USER` | `3` | 1 through 1000 | Maximum pending operations for one actor. |

Cross-field checks also require `SHUTDOWN_TIMEOUT_SECS > DELIVERY_TIMEOUT_SECS + DOWNLOAD_HEARTBEAT_SECS` and `WORKER_PROCESSING_TIMEOUT_SECS <= DOWNLOAD_TIMEOUT_SECS` while media is enabled. A workspace path must be absolute and not `/`. Numeric values are bounded unsigned integers. The private-origin exception is not a general SSRF bypass: the worker still checks resolved destinations and redirect destinations.

## Localization

Telegram replies use embedded [English](../../backend/crates/yoink-i18n/locales/en.toml) and [Russian](../../backend/crates/yoink-i18n/locales/ru.toml) TOML catalogs. Update both catalogs together. Keys and named `{parameter}` sets must match the typed manifest in `yoink-i18n`; invalid catalogs prevent Telegram startup. Templates are compiled once and inserted values are never interpreted as templates. Telegram language tags select Russian for `ru` and supported regional forms, with English as the fallback. Resource changes require a rebuild.

## Diagnostics and secrets

HTTP endpoints are unauthenticated diagnostics without user data. Keep the loopback bind unless exposing them is intentional. `/health/ready` returns 503 during startup and draining; readiness does not certify product functionality. Tokens and database URLs are redacted in configuration diagnostics. Use an approved secret delivery mechanism, not committed files or command output.
