# yoink-core

Telegram bot platform with a plugin system, REST API, and React WebApp. Handles auth, RBAC, groups, settings, and provides a plugin protocol that each plugin implements.

Plugins are git submodules under `plugins/`. Each is an independent Python package with its own handlers, routes, models, migrations, and frontend pages.

## Quick start

```bash
cp .env.example .env        # fill in required vars
just data-dirs              # create data/ subdirectories
just build all              # build yoink + frontend images
just up                     # start services
just migrate up             # run migrations
```

## Services

| Service | Description | Port |
|---|---|---|
| `yoink` | Bot + API | 8003 |
| `yoink-postgres` | PostgreSQL 17 | - |
| `yoink-frontend` | React SPA (nginx) | 3010 |
| `yoink-tg-bot-api` | Custom tdlight Bot API server | 8082 |
| `yoink-backup` | pg_dump + S3 (profile `backup`) | - |
| `yoink-browser` | Kasmweb Chromium (profile `cookies`) | 6902 |

## Just recipes

```
just build [yoink|frontend|tg|backup|all]
just up [service]
just down
just restart <service>
just logs [service]
just ps
just migrate [up|down|current|history|create "msg"]
just psql
just shell [service]
just test [path]
just data-dirs
just clean-pyc
just reset
just tg-login +<phone>
just tg-status
just tg-logout
just browser [up|down|logs]
just proxy-init
just backup
just restore [list|latest|FILE]
just backup-up / backup-down / backup-logs
```

## Environment variables

Core variables (all lowercase):

| Variable | Required | Default | Description |
|---|---|---|---|
| `bot_token` | yes | - | Telegram bot token |
| `owner_id` | yes | - | Telegram user ID of the owner |
| `api_id` / `api_hash` | yes | - | Telegram API credentials (for tg-bot-api) |
| `api_secret_key` | yes | - | JWT signing secret |
| `yoink_plugins` | no | - | Comma-separated plugin names (default: empty) |
| `database_url` | no | - | PostgreSQL URL (default provided) |
| `telegram_base_url` | no | - | Bot API base URL (default: official Telegram) |
| `data_dir` | no | - | Host path for cookies, sessions, browser profile |
| `json_logs` | no | - | Enable JSON log format |
| `DEV_AUTH_ENABLED` | no | `false` | Enable /auth/dev endpoint (restricted to DEV_ALLOWED_CIDR by nginx) |
| `DEV_ALLOWED_CIDR` | no | `192.168.0.0/16` | CIDR allowed to access /auth/dev (nginx-enforced) |

See `.env.example` for the full list with defaults. Plugin-specific variables are documented in each plugin's README.

## REST API

Base path: `/api/v1`. Docs: `http://localhost:8003/docs`.

Auth: `Authorization: Bearer <JWT>` obtained via `POST /api/v1/auth/token` (Telegram WebApp initData).

### Core endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | /auth/token | - | Exchange Telegram initData for JWT |
| POST | /auth/dev | - | Dev token (local network only, requires DEV_AUTH_ENABLED=true) |
| GET | /users/me | user | Current user profile |
| GET | /users | admin | List all users |
| PATCH | /users/{id} | admin | Update role |
| GET | /users/{user_id}/photo | public | Proxy user avatar from Bot API |
| POST | /users/photos/sync | owner | Mass-backfill user avatars from Bot API |
| GET | /groups | admin | List groups |
| PATCH | /groups/{id} | admin | Update group settings |
| GET | /settings | user | Personal settings |
| PATCH | /settings | user | Update settings |
| GET | /bot-settings | admin | Global bot settings |
| PATCH | /bot-settings | owner | Update global settings |
| GET | /permissions | admin | List all user permissions |
| GET | /permissions/{uid} | admin | Permissions for a user |
| POST | /permissions/{uid} | admin | Grant a feature |
| DELETE | /permissions/{uid}/{feature} | admin | Revoke a feature |
| GET | /features | user | List all registered features |

### M2M API

Auth: `X-Api-Key` header. Base path: `/api/internal/v1`.

| Method | Path | Scope | Description |
|---|---|---|---|
| GET | /status | health:r | Bot status |
| GET | /users | users:r | List users |
| GET | /groups | groups:r | List groups |
| POST | /events | events:w | Create event |

API key management: `GET/POST /api/v1/api-keys`, `DELETE /api/v1/api-keys/{id}` (owner only).

### Health

| Method | Path | Description |
|---|---|---|
| GET | /health | Health check (DB / bot / disk) |
| GET | /metrics | In-process counters |

Plugin routes are mounted at `/api/v1/{plugin_name}/`.

## Dev authentication

`POST /api/v1/auth/dev` generates a JWT for any user_id without Telegram verification.

- Requires `DEV_AUTH_ENABLED=true` in env
- **Restricted to `DEV_ALLOWED_CIDR` at nginx level** - never reachable from the internet
- Accepts `user_id` and `role` query params (default role: `user`)
- Used by the frontend when `?dev_token=<user_id>:<role>` is in the URL or `VITE_DEV_TOKEN` is set in `.env.local`

## Plugin system

Plugins are Python packages declared via entry points:

```toml
[project.entry-points."yoink.plugins"]
dl = "yoink_dl:DlPlugin"
```

Each plugin implements the `YoinkPlugin` protocol:

| Method | Returns | Description |
|---|---|---|
| `get_handlers()` | `list[HandlerSpec]` | PTB message/command handlers |
| `get_inline_handlers()` | `list[InlineHandlerSpec]` | Inline query handlers |
| `get_routes()` | `APIRouter \| None` | FastAPI routes |
| `get_models()` | `list` | SQLAlchemy models |
| `get_locale_dir()` | `Path \| None` | Directory with `en.yml`, `ru.yml` |
| `get_jobs()` | `list[JobSpec] \| None` | Scheduled background jobs |
| `get_web_manifest()` | `WebManifest \| None` | Frontend pages and sidebar entries |
| `get_commands()` | `list[CommandSpec]` | Bot commands for BotFather menu |
| `get_features()` | `list[FeatureSpec]` | RBAC features declared by this plugin |
| `get_help_section()` | `str` | HTML fragment for `/help` |
| `setup(ctx)` | - | Async startup: init services, populate `bot_data` |

### Inline dispatcher

Core registers a single `InlineQueryHandler` that routes queries to plugins by priority, then prefix, then pattern, then catch-all. If `access_policy` is set on an `InlineHandlerSpec`, access is checked before calling the handler - denied users are silently skipped.

| Plugin | Priority | Description |
|---|---|---|
| yoink-music | 10 | Music URLs and empty-query hint; returns `False` for non-music |
| yoink-dl | 0 | YouTube/URL search catch-all |

## RBAC

Role hierarchy: `banned < restricted < user < moderator < admin < owner`

Owner is set via `owner_id` env var at startup and always has full access.

### Features

Plugins declare `FeatureSpec` objects that describe access-gated capabilities:

```python
FeatureSpec(
    plugin="insight",
    feature="summary",
    label="AI Summary",
    description="Access to /summary and /about commands",
    default_min_role=None,  # None = explicit grant required; "user" = all users by default
)
```

Access is granted if **either**:
1. `user.role >= feature.default_min_role` (role threshold), or
2. An explicit grant exists in `user_permissions` table

Owner always passes regardless.

### Commands

`CommandSpec` supports `required_feature="plugin:feature"` to hide a command from users who don't have access:

```python
CommandSpec(
    command="summary",
    description="Summarize a YouTube video",
    required_feature="insight:summary",
)
```

Bot command menus are refreshed automatically on role change, grant/revoke, language change, and `/start`.

### Web dashboard

- `/admin/users` - user list with role management; Item list + bottom Drawer with tabs; user avatars loaded from photo proxy
- `/admin/groups` - group list; Item list + Dialog for editing; thread policies via badge + ThreadPoliciesDialog
- `/admin/permissions` - per-feature access matrix (grant/revoke per user)
- `/admin/bot-settings` - accepts plugin-contributed sections via `PluginManifest.botSettingsSections`

Both admin pages support dynamic search with 300 ms debounce (opacity fade, no skeleton flash).

## Database migrations

Single Alembic chain covering core and all plugins:

| Migration | Description |
|---|---|
| 0001_initial_schema | users, groups, thread_policies, bot_settings, events |
| 0002_dl_plugin_schema | dl settings, file cache, download log, rate limits, cookies |
| 0003_stats_plugin_schema | message log, stats tables |
| 0004_stats_tsvector | full-text search index |
| 0005_group_storage | inline storage settings |
| 0006_user_is_premium | premium flag |
| 0007_dl_dm_topic | DM topic thread ID |
| 0008_api_keys | M2M API keys |
| 0009_insight_plugin_schema | insight_access table |
| 0010_file_cache_multi | file_ids JSON column for album/gallery results |
| 0011_gallery_zip | gallery_zip flag in download log |
| 0012_user_permissions | unified user_permissions table |
| 0013_insight_user_settings | insight_user_settings table |
| 0014_dl_download_log_fields | clip_start, clip_end, media_type, group_title in download_log |
| 0015_dl_cookies_inherited | inherited flag in cookies table |
| 0016_music_download_log | user_id, group_id, thread_id in music download_log entries |
| 0017_stats_ranked_list | ranked list and period fields for stats |
| 0018_user_photo_url | photo_url column in users table |
| 0019_file_cache_key_length | file_cache.cache_key String(64) → String(80) |
| 0020_cookie_pool_flag | is_pool BOOLEAN in cookies table + index |
| 0021_cookie_label | Cookie.label String(128) column |
| 0022_cookie_personal_unique | partial unique index on cookies WHERE is_pool=false |
| 0023_cookie_avatar_url | Cookie.avatar_url String(512) |
| 0024_cookie_content_hash | Cookie.content_hash String(64) + index |
| 0025_cookie_session_key | Cookie.session_key String(256) + index |
| 0026_user_settings_use_pool | dl_user_settings.use_pool_cookies BOOLEAN DEFAULT TRUE |

## Custom Bot API server

Built from [tdlight-telegram-bot-api](https://github.com/tdlight-team/tdlight-telegram-bot-api) with `docker/patches/tdlight-forum-extras.patch`. Adds methods for forum topics, message viewers, chat history, etc.

```bash
just build tg   # ~10 min first build, cached thereafter
```

Current server version: **9.5**.

## User-mode session

The bot API server runs with `--allow-users`, enabling both bot and user accounts. The user session unlocks API methods unavailable to bots (forum topic listing, chat history, etc).

```bash
just tg-login +79001234567
just tg-status
just tg-logout
```

Token stored in `data/tg-bot-api/user.token`.

## Cookie extraction

```bash
just browser up   # start Kasmweb Chromium on port 6902
# log in to sites in the browser
just browser down
```

Cookies written to `data/cookies/` and picked up by yt-dlp. Alternatively, use the [browser extension](browser-extension/) to sync cookies directly from your own browser.

## Backup

Requires `backup_s3_*` env vars. pg_dump with custom format, uploaded to S3 via mc.

```bash
just backup           # one-shot
just restore list     # list available backups
just restore          # restore latest
just backup-up        # start cron (daily at 03:00)
```

Retention: 7 daily + 4 weekly.

## Tests

```bash
just test                        # core tests
cd plugins/yoink-dl && just test
cd plugins/yoink-stats && just test
```
