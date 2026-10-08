# Configuration

The native Rust server uses the embedded Turso 0.7.2 engine through the `turso` crate, not libsql. It does not use PostgreSQL or SQLx. Yoink reads explicit `YOINK_RUST_*` environment variables and does not load `.env` or legacy Python configuration. Python and Rust settings configure separate runtimes. Defaults keep authentication, Telegram, media, and database access disabled, with HTTP bound to loopback.

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

## Optional frontend authentication

Authentication requires an existing migrated native database. It can run without Telegram polling. Enable it with explicit credentials for the same bot that supplies the frontend's WebApp initData. No secret is generated or inherited from the Python runtime.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_AUTH_ENABLED` | `false` | Exact `true` or `false` | Enables frontend identity/authentication routes. Requires database path, bot token, API secret and owner ID. |
| `YOINK_RUST_API_SECRET_KEY` | unset | At least 32 UTF-8 bytes | Explicit HS256 signing secret. Keep private and stable across restarts. |
| `YOINK_RUST_AUTH_INIT_DATA_MAX_AGE_SECS` | `86400` | 1 through 86400 seconds | Maximum age of signed WebApp data. |
| `YOINK_RUST_AUTH_FUTURE_SKEW_SECS` | `30` | 0 through 300 seconds | Allowed future clock skew for WebApp data. |
| `YOINK_RUST_API_TOKEN_EXPIRE_MINUTES` | `1440` | 1 through 10080 minutes | JWT lifetime. Tokens contain `sub`, `role` and `exp`. |
| `YOINK_RUST_DEV_AUTH_ENABLED` | `false` | Exact `true` or `false` | Requires authentication enabled. Adds unauthenticated existing-account impersonation for local development. Never enable on an exposed listener. |

`POST /api/v1/auth/token` accepts JSON with `init_data`. Verification decodes form values once, checks the Telegram WebAppData HMAC, requires `auth_date`, and bounds timestamps and payload size. Registration uses the persisted `bot_access_mode`: missing means `open`, `approved_only` creates restricted users, and invalid configured values fail closed. New configured-owner accounts persist the owner role. Existing accounts retain stored roles, bans, preferences and grants while signed profile metadata is refreshed; the configured owner is elevated in the returned profile only after blocking checks.

Authenticated `GET /api/v1/users/me` and `GET/PATCH /api/v1/settings` use a bearer token and reload current database identity and ban state. Settings expose `user_id`, `language` and `theme`; updates accept `en`/`ru` and Catppuccin `latte`/`frappe`/`macchiato`/`mocha`. JWT role claims do not authorize operations. Auth routes reject requests while runtime readiness is false, including failed startup and shutdown. Successful responses and application errors use `Cache-Control: no-store`.

With the separate dev flag, `POST /api/v1/auth/dev?user_id=ID` issues a token only for an existing unblocked account; a supplied role cannot elevate it. Without that flag the route does not exist. These routes are the authentication prerequisite, not complete frontend/API parity. User statistics, account administration and product endpoints remain separate work.

## Optional Telegram adapter

Telegram is disabled unless enabled explicitly.

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_TELEGRAM_ENABLED` | `false` | Exact `true` or `false` | Controls Telegram adapter startup. |
| `YOINK_RUST_TELEGRAM_GROUPS_ENABLED` | `false` | Exact `true` or `false` | Enables group/topic commands for known authorized users when media is enabled. Does not register users or groups. |
| `YOINK_RUST_TELEGRAM_TOKEN` | unset | Required when enabled; non-empty, no whitespace | Bot token. Treat as a secret. |
| `YOINK_RUST_OWNER_ID` | unset | Positive ID up to signed 64-bit maximum | Required configured owner identity. |
| `YOINK_RUST_BOT_API_URL` | unset | HTTP(S) origin without credentials, path, query, or fragment | Optional Bot API endpoint override. |
| `YOINK_RUST_TELEGRAM_POLL_TIMEOUT_SECS` | `30` | 1 through 50 seconds | Long-poll timeout. |
| `YOINK_RUST_TELEGRAM_REQUEST_TIMEOUT_SECS` | `60` | 1 through 120 seconds | Total ordinary Bot API request deadline, including response-body reading. Prevents stalled generic replies retaining handler ownership forever. Raw polling keeps its separate twice-poll deadline. |
| `YOINK_RUST_TELEGRAM_COOKIE_UPLOAD_TIMEOUT_SECS` | `30` | 1 through 60 seconds | Deadline for awaiting Bot API metadata and document content. The file limit is 512 KiB. Local kernel I/O cannot be cancelled, but only one such read can remain outstanding. |
| `YOINK_RUST_COOKIE_IMPORT_TIMEOUT_SECS` | `30` | 1 through 60 seconds | Total deadline for cookie URL imports, including concurrency wait, DNS, redirects and bounded body streaming. |
| `YOINK_RUST_COOKIE_IMPORT_PRIVATE_ORIGINS` | `[]` | JSON array of at most 16 exact HTTP(S) literal-IP origins with explicit nonzero ports | Explicit exceptions to the cookie-import public-address policy. No hostname, credentials, path, query, fragment or wildcard. Leave empty unless that private endpoint is deliberately trusted. |
| `YOINK_RUST_BOT_API_FILE_ROOT` | unset | Absolute, non-root directory without traversal | Remote prefix returned by local Bot API `getFile`. Requires both the local root below and an explicit `BOT_API_URL`. |
| `YOINK_RUST_TELEGRAM_LOCAL_FILE_ROOT` | unset | Absolute, non-root directory without traversal | Read-only mount corresponding to the remote prefix. Local file access is disabled unless both roots are set. Symlinks and non-regular files are rejected. |
| `YOINK_RUST_TELEGRAM_PROGRESS_INTERVAL_SECS` | `2` | 1 through 60 seconds | Global progress tick. At most one status-message edit per tick, coalescing unchanged text. |
| `YOINK_RUST_TELEGRAM_PROGRESS_TIMEOUT_SECS` | `10` | 1 through 60 seconds | Deadline for editing an already-confirmed status message. Ambiguous edits are safe to retry, unlike media sends. |

With a native database configured, trusted private messages register missing users through the shared open/approved-only policy. Existing profiles keep their roles, bans, preferences and grants. Ordinary authorized users can use `/start`, `/help`, basic `/download`, `/video`, `/audio`, `/image`, visible plain video URLs, and their scoped `/job` and `/cancel` operations. Extended media flags, playlists, clips, subtitles and all private settings/credential controls remain configured-owner-only, as does `/status`. Without a database, private diagnostics remain configured-owner-only. Groups require the separate opt-in above, an identifiable non-bot sender, and current authorization for the original group/topic. The media service checks identity and destination at admission, execution, delivery and progress. Disabling group access prevents persisted group jobs from starting and retires their progress notices. See [access control](access-control.md) for role precedence and current limits. Database files have no PostgreSQL-style column or role ACLs: the trusted server process can read every row, including credentials. Treat the process, its host, and the private state directory as the security boundary. Read snapshots must not expose credential contents through operator or user-facing APIs.

Cookie URL imports use a separate direct HTTP client. They do not send Telegram tokens, saved cookies, proxy credentials or ambient HTTP-proxy configuration to the source. Every redirect is resolved and checked again, and HTTPS cannot redirect to HTTP. The private-origin exception above is independent of the worker's origin configuration and does not permit other ports or redirect destinations. Adding an origin lets the owner request paths on that internal endpoint, so do not list a privileged local API. HTTP source URLs transfer cookie secrets in plaintext. Prefer HTTPS or upload a Telegram document instead.

## Optional media downloads

Downloads are disabled unless enabled explicitly. Enabling them requires Telegram and a database path, plus the opt-in media fixture image and its test-only controller configuration for test runs. The default production runtime image does not contain the fixture controller. The fixed worker launcher enforces Landlock before starting the isolated Python worker. See [media slice](media-slice.md) for exact fixture-image usage and limitations.

The supported download, timeout, workspace, proxy, credential, quota, retry, cache and worker-output settings remain under the `YOINK_RUST_DOWNLOAD_*` and `YOINK_RUST_WORKER_*` prefixes. Their defaults and validation are enforced in `backend/crates/yoink-server/src/media_config.rs`; consult that source when setting detailed values. Do not put database or bot credentials into committed files or example commands. The private-origin exception is only for a trusted, isolated local fixture.

### Download quotas

| Variable | Default | Supported value | Behavior |
| --- | --- | --- | --- |
| `YOINK_RUST_DOWNLOAD_QUOTA_MINUTE` | `5` | 1 through 1000000 | Accepted new operations per actor in an anchored 60-second window. |
| `YOINK_RUST_DOWNLOAD_QUOTA_HOUR` | `30` | 1 through 1000000 | Accepted new operations per actor in an anchored 3600-second window. |
| `YOINK_RUST_DOWNLOAD_QUOTA_DAY` | `100` | 1 through 1000000 | Accepted new operations per actor in an anchored 86400-second window. |
| `YOINK_RUST_DOWNLOAD_MAX_ACTIVE_PER_USER` | `3` | 1 through 100 | Running plus delivering operations per actor, across bots and destinations. Includes the owner and cache hits. |

Time quotas use preserved native/imported `rate_limits` rows and survive restart. Only `YOINK_RUST_OWNER_ID` bypasses time quotas; stored roles do not. Replay, conflicts and rejected admission do not consume counters. Retries do not charge again. Active work and pending admission capacity are separate limits. See the [quota contract](downloads-parity.md#quotas-and-active-work) for charging, reset and rollback behavior.

## Localization and diagnostics

Telegram replies use embedded English and Russian TOML catalogs. Update both catalogs together. Health and system diagnostics remain unauthenticated and contain no user data. Enabled auth routes expose the profile/settings contracts above. Keep the loopback bind unless exposure is intentional, use a trusted TLS reverse proxy for frontend traffic, and keep dev authentication disabled. Tokens are redacted in configuration diagnostics. Changes to embedded catalogs require a rebuild.
