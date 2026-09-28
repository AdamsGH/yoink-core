# Configuration

The native Rust server uses the embedded Turso 0.7.2 engine through the `turso` crate, not libsql. It does not use PostgreSQL or SQLx. Yoink reads explicit `YOINK_RUST_*` environment variables and does not load `.env` or legacy Python configuration. Python and Rust settings configure separate runtimes. Defaults keep Telegram, media, and database access disabled, with HTTP bound to loopback.

## Server settings

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_HTTP_BIND` | `127.0.0.1:8088` | Socket address | HTTP diagnostics listener. |
| `YOINK_RUST_STARTUP_TIMEOUT_SECS` | `15` | 1 through 300 seconds | Dependency startup deadline. |
| `YOINK_RUST_SHUTDOWN_TIMEOUT_SECS` | `10` without media; `60` with media | 1 through 300 seconds | Graceful adapter and database drain deadline. Media requires this to exceed delivery timeout plus heartbeat interval. |

## Native database

Set `YOINK_RUST_DATABASE_PATH` to an absolute path whose parent directory already exists and is private. On Unix, the parent must be owned/controlled as a private directory with permissions no broader than `0700`. The application does not create the parent directory. On Unix, it creates the database file with mode `0600` during explicit `migrate`; it may create the sidecar lock file with mode `0600` when opening the database. Provision the private parent directory with mode `0700`. Normal server startup and `check-schema` require an existing database and never create or migrate it.

The database is owned by one server process at a time. A sidecar `.yoink-lock` file provides an advisory exclusive lock. The runtime serializes access through one connection. Do not share the database file with another application or process. Unix health checks verify database and lock inode identity and parent-directory permissions, so replacement, removal, or permission drift makes the database unhealthy.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_DATABASE_PATH` | unset | Absolute local file path with an existing private parent | Enables database-backed readiness and services. |
| `YOINK_RUST_DATABASE_ACQUIRE_TIMEOUT_SECS` | `5` | 1 through 300 seconds | Maximum wait to acquire the serialized connection. |
| `YOINK_RUST_DATABASE_HEALTH_INTERVAL_SECS` | `5` | 1 through 300 seconds | Interval between schema and file-health checks. |

Turso also needs writable scratch storage for query execution. In 0.7.2, temporary database directories honor `TURSO_TMPDIR`, then `SQLITE_TMPDIR`, then the OS temporary directory. Ephemeral query files use the OS temporary directory directly. For a read-only Linux container, mount a separate owner-only tmpfs at `/tmp/yoink-db` and set **both** `TMPDIR=/tmp/yoink-db` and `TURSO_TMPDIR=/tmp/yoink-db`. Setting only the Turso override leaves some queries trying to write to read-only `/tmp`. These variables select scratch storage, not the persistent database location. Never use the worker workspace for database scratch files. The executor clears the child's environment and Landlock excludes this directory. See the hardened container example in the media guide.

The legacy `DATABASE_URL`, `MAX_CONNECTIONS`, `YOINK_RUST_DATABASE_URL`, and database max-connection settings are rejected. There is no host PostgreSQL configuration path in this runtime.

Run `yoink-server migrate` explicitly against the intended file to create a missing database and apply ordered, idempotent upgrades. It does not create the parent directory. `yoink-server check-schema` and ordinary startup are read-only and reject missing, malformed, drifted, unknown, or checksum-mismatched history. See [persistence](persistence.md) and [development](development.md).

## Optional Telegram adapter

Telegram is disabled unless enabled explicitly.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_TELEGRAM_ENABLED` | `false` | Exact `true` or `false` | Controls Telegram adapter startup. |
| `YOINK_RUST_TELEGRAM_TOKEN` | unset | Required when enabled; non-empty, no whitespace | Bot token. Treat as a secret. |
| `YOINK_RUST_OWNER_ID` | unset | Positive ID up to signed 64-bit maximum | Required configured owner identity. |
| `YOINK_RUST_BOT_API_URL` | unset | HTTP(S) origin without credentials, path, query, or fragment | Optional Bot API endpoint override. |
| `YOINK_RUST_TELEGRAM_POLL_TIMEOUT_SECS` | `30` | 1 through 50 seconds | Long-poll timeout. |
| `YOINK_RUST_TELEGRAM_PROGRESS_INTERVAL_SECS` | `2` | 1 through 60 seconds | Global progress tick. At most one status-message edit per tick, coalescing unchanged text. |
| `YOINK_RUST_TELEGRAM_PROGRESS_TIMEOUT_SECS` | `10` | 1 through 60 seconds | Deadline for editing an already-confirmed status message. Ambiguous edits are safe to retry, unlike media sends. |

The outer command gate remains restricted to the configured owner in a private chat. The media service separately uses an authorized identity snapshot for admission, execution, and delivery. Database files have no PostgreSQL-style column or role ACLs: the trusted server process can read every row, including credentials. Treat the process, its host, and the private state directory as the security boundary. Read snapshots must not expose credential contents through operator or user-facing APIs.

## Optional media downloads

Downloads are disabled unless enabled explicitly. Enabling them requires Telegram and a database path, plus the opt-in media fixture image and its test-only controller configuration for test runs. The default production runtime image does not contain the fixture controller. The fixed worker launcher enforces Landlock before starting the isolated Python worker. See [media slice](media-slice.md) for exact fixture-image usage and limitations.

The supported download, timeout, workspace, proxy, credential, quota, retry, cache and worker-output settings remain under the `YOINK_RUST_DOWNLOAD_*` and `YOINK_RUST_WORKER_*` prefixes. Their defaults and validation are enforced in `backend/crates/yoink-server/src/media_config.rs`; consult that source when setting detailed values. Do not put database or bot credentials into committed files or example commands. The private-origin exception is only for a trusted, isolated local fixture.

## Localization and diagnostics

Telegram replies use embedded English and Russian TOML catalogs. Update both catalogs together. HTTP endpoints are unauthenticated diagnostics without user data. Keep the loopback bind unless exposing diagnostics is intentional. Tokens are redacted in configuration diagnostics. Changes to embedded catalogs require a rebuild.
