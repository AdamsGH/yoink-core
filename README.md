# yoink-core

Telegram bot for media downloading, music link aggregation, and group chat analytics. Modular plugin architecture - each plugin is an independent Python package and git submodule.

## Plugins

| Plugin | Submodule | Description |
|---|---|---|
| [yoink-dl](plugins/yoink-dl) | `plugins/yoink-dl` | Media downloader (yt-dlp + gallery-dl) |
| [yoink-music](plugins/yoink-music) | `plugins/yoink-music` | Music link aggregator (Spotify / Deezer / YM / YTM / SoundCloud / Apple Music) |
| [yoink-stats](plugins/yoink-stats) | `plugins/yoink-stats` | Group chat analytics with web dashboard |

Enable plugins via `yoink_plugins=dl,music,stats` in `.env`.

## Quick start

```bash
cp .env.example .env       # fill in bot_token, owner_id, api_id, api_hash, api_secret_key
just data-dirs             # create data/ subdirectories
just build all             # build yoink + frontend images
just up                    # start services
just migrate up            # run migrations
```

## Services

| Service | Description | Port |
|---|---|---|
| `yoink` | Bot + API | 8003 |
| `yoink-postgres` | PostgreSQL 17 | - |
| `yoink-frontend` | React SPA (nginx) | 3010 |
| `yoink-tg-bot-api` | Custom tdlight Bot API server | 8082 |
| `yoink-backup` | pg\_dump + S3 (profile `backup`) | - |
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

| Variable | Required | Description |
|---|---|---|
| `bot_token` | yes | Telegram bot token |
| `owner_id` | yes | Telegram user ID of the owner |
| `api_id` / `api_hash` | yes | Telegram API credentials (for tg-bot-api) |
| `api_secret_key` | yes | JWT signing secret |
| `yoink_plugins` | no | Comma-separated plugin names (default: empty) |
| `database_url` | no | PostgreSQL URL (default provided) |
| `telegram_base_url` | no | Bot API URL (default: official Telegram) |
| `data_dir` | no | Host path for cookies, sessions, browser profile |
| `json_logs` | no | Enable JSON log format |

See `.env.example` for the full list with defaults. Plugin-specific variables are documented in each plugin's README.

## REST API

Base path: `/api/v1`. Docs (Scalar): `http://localhost:8003/docs`.

Auth: `Authorization: Bearer <JWT>` obtained via `POST /api/v1/auth/token` (Telegram WebApp initData).

### Core endpoints

| Method | Path | Auth | Description |
|---|---|---|---|
| POST | /auth/token | - | Exchange Telegram initData for JWT |
| GET | /users/me | user | Current user |
| GET | /users | admin | List users |
| PATCH | /users/{id} | admin | Update role/ban |
| GET | /groups | admin | List groups |
| PATCH | /groups/{id} | admin | Update group settings |
| GET | /settings | user | Personal settings |
| PATCH | /settings | user | Update settings |
| GET | /bot-settings | admin | Global settings |
| PATCH | /bot-settings | owner | Update global settings |

### Forum and message proxy (owner, requires user session)

| Method | Path | Description |
|---|---|---|
| GET | /forum/topics/{chat\_id} | List topics |
| GET | /forum/topics/{chat\_id}/{thread\_id} | Single topic |
| GET | /forum/topics/{chat\_id}/{thread\_id}/link | Public link |
| GET | /forum/search/{chat\_id} | Search messages |
| GET | /forum/history/{chat\_id} | Chat history |
| GET | /messages/{chat\_id}/{msg\_id}/viewers | Message viewers |
| GET | /messages/{chat\_id}/{msg\_id}/link | Shareable link |
| GET | /messages/{chat\_id}/{msg\_id}/read-date | Read receipt |
| GET | /messages/{chat\_id}/{msg\_id}/thread | Thread info |
| GET | /messages/{chat\_id}/by-date | Find message by timestamp |

### M2M API

Auth: `X-Api-Key` header (SHA-256 hashed, scope-based). Base path: `/api/internal/v1`.

| Method | Path | Scope | Description |
|---|---|---|---|
| GET | /status | health:r | Bot status |
| GET | /users | users:r | List users |
| GET | /groups | groups:r | List groups |
| POST | /events | events:w | Create event |

API key management: `GET/POST /api/v1/api-keys`, `DELETE /api/v1/api-keys/{id}` (owner, JWT auth).

### Health

| Method | Path | Description |
|---|---|---|
| GET | /health | Health check (DB / bot / disk) |
| GET | /metrics | In-process counters |

Plugin routes are mounted at `/api/v1/{plugin_name}/`.

## Plugin system

Plugins are Python packages declared via entry points:

```toml
[project.entry-points."yoink.plugins"]
dl = "yoink_dl:DlPlugin"
```

Each plugin implements the `YoinkPlugin` protocol and can provide:

- PTB message/command handlers (`get_handlers()`)
- Inline query handlers (`get_inline_handlers()`) - registered via the core inline dispatcher
- FastAPI routes (`get_routes()`)
- SQLAlchemy models (`get_models()`)
- Locale files (`get_locale_dir()`)
- Scheduled jobs (`get_jobs()`)
- Frontend web manifest (`get_web_manifest()`)

### Inline dispatcher

Core registers a single `InlineQueryHandler` that routes queries to plugins by priority, then prefix, then pattern, then catch-all. Plugins declare `InlineHandlerSpec(callback, priority, prefix, pattern, access_policy)`. If `access_policy` is set, the dispatcher runs `PermissionChecker` before calling the handler - denied users are silently skipped.

Current specs (descending priority):

| Plugin | Priority | Match | Description |
|---|---|---|---|
| yoink-music | 10 | catch-all | Shows paste hint on empty query; returns `False` for non-music text so dl still handles YouTube search |
| yoink-dl | 0 | catch-all | YouTube/URL search |

## Database migrations

Single Alembic chain covering core and all plugins:

```
0001_initial_schema        - users, groups, thread_policies, bot_settings, events
0002_dl_plugin_schema      - dl settings, file cache, download log, rate limits, cookies
0003_stats_plugin_schema   - message log, stats tables
0004_stats_tsvector        - full-text search index
0005_group_storage         - inline storage settings
0006_user_is_premium       - premium flag
0007_dl_dm_topic           - DM topic thread ID
0008_api_keys              - M2M API keys
```

## Role hierarchy

`banned < restricted < user < moderator < admin < owner`

Owner is set via `owner_id` env var at startup, never stored in the database.

## Custom Bot API server

Built from [tdlight-telegram-bot-api](https://github.com/tdlight-team/tdlight-telegram-bot-api) with `docker/patches/tdlight-forum-extras.patch`. Adds 16 methods for forum topics, message viewers, chat history, etc.

```bash
just build tg   # ~10 min first build, cached thereafter
```

## User-mode session

The bot API server runs with `--allow-users`, enabling both bot and user accounts. The user session unlocks API methods unavailable to bots (forum topic listing, chat history, etc).

```bash
just tg-login +79001234567
just tg-status
just tg-logout
```

Token stored in `data/tg-bot-api/user.token` (chmod 600).

## Cookie extraction

```bash
just browser up   # start Kasmweb Chromium on port 6902
# log in to sites in the browser
just browser down
```

Cookies written to `data/cookies/` and picked up by yt-dlp. Alternatively, use the [browser extension](browser-extension/) to sync cookies directly from your own browser.

## Backup

Requires `backup_s3_*` env vars. pg\_dump with custom format, uploaded to S3 via mc.

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
