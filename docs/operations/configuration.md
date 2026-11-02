# Configuration

The Rust binary reads explicit `YOINK_RUST_*` environment variables. It does not load `.env` or Python settings. The supplied Compose stack loads the private `.env` and maps selected shared values into native names.

Use [.env.example](../../.env.example) as a deployment template, not a complete native configuration reference. It contains both current and legacy settings.

## Standalone and Compose defaults

| Setting | Standalone default | Supplied Compose deployment |
| --- | --- | --- |
| HTTP | `127.0.0.1:8088` | Container `0.0.0.0:8000`, host port 8003. |
| Database | Disabled | `/state/native.db` in `data/native`. |
| Telegram | Disabled | Enabled with the shared bot token and local Bot API. |
| Authentication | Disabled | Enabled with the shared signing secret and owner ID. |
| Downloads, Music, Stats, Insight, Inbox | Disabled | Explicitly enabled. |
| Groups | Disabled | Explicitly enabled, subject to current group/topic policy. |

The [Compose file](../../docker-compose.yaml) defines deployment overrides. The [typed server configuration](../../backend/crates/yoink-server/src/config.rs) defines validation and standalone defaults.

## Core credentials

| Compose input | Native setting | Requirement |
| --- | --- | --- |
| `bot_token` | `YOINK_RUST_TELEGRAM_TOKEN` | Token for the deployed bot. |
| `owner_id` | `YOINK_RUST_OWNER_ID` | Positive Telegram user ID. |
| `api_secret_key` | `YOINK_RUST_API_SECRET_KEY` | At least 32 UTF-8 bytes. |
| `api_id`, `api_hash` | Local Bot API service credentials | Required by the supplied TDLight service. |
| `yoink_domain` | Cookie WebApp HTTPS origin and edge configuration | Hostname without a scheme. |

Do not copy a legacy optional key and assume Rust reads it. For example, native Spotify credentials use `YOINK_RUST_MUSIC_SPOTIFY_CLIENT_ID` and `YOINK_RUST_MUSIC_SPOTIFY_CLIENT_SECRET`, not `MUSIC_SPOTIFY_CLIENT_ID`.

## Database and scratch storage

`YOINK_RUST_DATABASE_PATH` must be an absolute path in an existing private directory. On Unix, the directory must have permissions no broader than `0700`. Database and lock files use private permissions.

One process owns the database. Do not share its live file with a second runtime or inspect it through another database owner.

The standalone server verifies the existing schema without migration. Explicit `migrate` creates or upgrades the database. The container entrypoint also verifies without migration. An operator must run migration explicitly before starting a new or upgraded schema.

Set both `TMPDIR` and `TURSO_TMPDIR` to writable database scratch storage when the container root is read-only. Worker workspaces must remain separate from database scratch storage.

Uppercase `DATABASE_URL`, `MAX_CONNECTIONS`, and old native PostgreSQL settings are rejected. Lowercase legacy `database_url` does not configure Turso.

## Authentication and authorization

`YOINK_RUST_AUTH_ENABLED=true` requires a migrated database, bot token, owner ID, and signing secret. Telegram WebApp initData supplies the authenticated identity. Subsequent requests use a bearer token.

Services reload current roles, bans, and grants from the database. A role claim in a token cannot override a demotion or ban.

`YOINK_RUST_DEV_AUTH_ENABLED` defaults to `false`. When enabled, the development route impersonates existing unblocked accounts only. It does not accept a role elevation. Never expose this route publicly or rely on a legacy lowercase flag.

See [API authentication](../architecture/api.md) and [access control](../architecture/access-control.md).

## Product configuration

| Product | Enable flag | Reference |
| --- | --- | --- |
| Downloads | `YOINK_RUST_DOWNLOADS_ENABLED` | [Download settings](../features/downloads/settings.md), [worker configuration](../../backend/crates/yoink-server/src/media_config.rs). |
| Music | `YOINK_RUST_MUSIC_ENABLED` | [Music](../features/music.md), [product limits](../architecture/products.md). |
| Stats | `YOINK_RUST_STATS_ENABLED` | [Stats](../features/stats.md), [search](../features/stats-search.md). |
| Insight | `YOINK_RUST_INSIGHT_ENABLED` | [Insight configuration](../features/insight/runtime.md). |
| Inbox | `YOINK_RUST_INBOX_ENABLED` | [Inbox runtime](../features/inbox/runtime.md). |

The enable flags do not supply provider credentials. Configure optional providers separately. Native AI requests require an explicit SOCKS5 proxy.

## Proxy routing and network access

Compose provides an HTTP facade at `http://yoink-proxy:8080`. Its upstream defaults to `socks5://host.docker.internal:1080`.

Downloader automatic domain routing is independent of the personal proxy toggle. `YOINK_RUST_DOWNLOAD_PROXY_DOMAINS_JSON` selects automatic domains. Personal proxy selection, pool access, and frozen credential references retain separate authorization checks.

Music uses `YOINK_RUST_MUSIC_PROXY_URL` and `YOINK_RUST_MUSIC_PROXY_PLATFORMS`. Insight uses `YOINK_RUST_INSIGHT_AI_PROXY` for AI traffic. These settings do not authorize arbitrary private source addresses.

Cookie URL import uses a separate guarded client. User-session transport uses its own configured origin and token file. See [cookies](../features/downloads/cookies.md) and [user sessions](user-session.md).

## Deadlines

Compose sets a 300-second startup deadline and a 150-second shutdown deadline. Docker grants 165 seconds for container shutdown.

Worker, provider, request, and database limits have separate bounds. Use the owning typed configuration rather than increasing a global deadline to conceal a failing dependency.
