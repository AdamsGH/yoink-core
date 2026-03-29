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
just tg login +<phone>
just tg status
just tg logout
just browser [up|down|logs]
just proxy-init
just backup
just backup restore [list|latest|FILE]
just backup [up|down|logs]
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
| GET | /users | admin | List all users (sortable: created_at, updated_at, name, role, dl_count, dl_last_at) |
| PATCH | /users/{id} | admin | Update role |
| GET | /users/{id}/stats | admin | Activity stats (dl + music + ai aggregated) |
| GET | /users/{user_id}/photo | public | Proxy user avatar from Bot API |
| POST | /users/photos/sync | owner | Mass-backfill user avatars from Bot API |
| GET | /groups | admin | List groups |
| POST | /groups | admin | Add group |
| PATCH | /groups/{id} | admin | Update group settings |
| GET | /groups/{id}/photo | public | Proxy group chat photo from Bot API |
| GET | /groups/{id}/threads | admin | List thread policies |
| POST | /groups/{id}/threads | admin | Add/toggle thread policy |
| DELETE | /groups/{id}/threads/{pid} | admin | Delete thread policy |
| GET | /threads/status | admin | Check if user-mode session is available |
| POST | /threads/scan/{group_id} | admin | Scan forum topics via user-mode session |
| GET | /settings | user | Personal settings |
| PATCH | /settings | user | Update settings |
| GET | /bot-settings | admin | Global bot settings |
| PATCH | /bot-settings | owner | Update global settings |
| GET | /bot-settings/tag-map | admin | Tag-to-feature map |
| PUT | /bot-settings/tag-map | owner | Update tag-to-feature map |
| GET | /bot-settings/available-features | admin | All registered features |
| GET | /permissions/all | admin | List all user permissions |
| POST | /users/{id}/permissions | admin | Grant a feature |
| DELETE | /users/{id}/permissions/{plugin}/{feature} | admin | Revoke a feature |
| GET | /users/{id}/feature-access | admin | Effective feature access for a user |
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
| `get_models()` | `list` | SQLAlchemy models (concrete classes, not base aliases) |
| `get_locale_dir()` | `Path \| None` | Directory with `en.yml`, `ru.yml` |
| `get_jobs()` | `list[JobSpec] \| None` | Scheduled background jobs |
| `get_web_manifest()` | `WebManifest \| None` | Frontend pages and sidebar entries |
| `get_commands()` | `list[CommandSpec]` | Bot commands for BotFather menu |
| `get_features()` | `list[FeatureSpec]` | RBAC features declared by this plugin |
| `get_help_section()` | `str` | HTML fragment for `/help` |
| `setup(ctx)` | - | Async startup: init services, populate `bot_data`, register `ActivityProvider` |

### Activity providers

Plugins register an `ActivityProvider` callable in `setup()` via `register_activity_provider()`. Core collects activity from all providers in `collect_activity()` to build `/users/{id}/stats` responses. This keeps core decoupled from plugin internals.

```python
# in plugin setup():
from yoink.core.activity import register_activity_provider
register_activity_provider("dl", dl_activity_provider)
```

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

## Web dashboard

The frontend is a React SPA served by nginx. Built with Vite + Tailwind + shadcn/ui components.

### Admin pages

- `/admin/users` - user list; Item list + bottom Drawer with tabs (Stats / Access / Edit); sortable by role, name, dl_count, dl_last_at; user avatars via photo proxy
- `/admin/groups` - group list; Item list + Dialog for editing; group photo via proxy; thread policies via Settings2 icon + ThreadPoliciesDialog; scan button (user-session only)
- `/admin/permissions` - per-feature access matrix (grant/revoke per user)
- `/admin/bot-settings` - accepts plugin-contributed sections via `PluginManifest.botSettingsSections`

Both list pages support dynamic search with 300 ms debounce (opacity fade, no skeleton flash).

### Frontend architecture

```
frontend/
  src/
    components/
      ui/            # shadcn components (never edited directly); index.ts barrel -> @ui alias
      app/           # app-level components (UserPanel, SettingRow, InlineSelect, StatusBadge); index.ts barrel -> @app alias
      charts/        # StatCard, PeriodToggle, chartColors; index.ts barrel -> @core/components/charts
    lib/
      api/           # typed API modules: users, groups, bot-settings, permissions, user-settings, threads
      api-client.ts  # axios instance with auth interceptor
      user-utils.ts  # userInitials, userPhotoUrl, GRADIENT/RING/roleMediaColor, openProfileLink
      utils.ts       # cn, formatDate, formatDateMonth, formatDateDay, formatBytes
    pages/
      admin/
        users/       # AdminUsersPage + UserDrawer (extracted)
        groups/      # AdminGroupsPage + useAdminGroups hook (extracted)
        ...
    types/
      api.ts         # core API types (User, Group, Feature, Permission, ...)
      plugin.ts      # PluginManifest, UserStats, NavGroup, ...
```

**Aliases:** `@core/*` = `frontend/src/*`, `@ui` = `frontend/src/components/ui`, `@app` = `frontend/src/components/app`, `@dl/*` = `plugins/yoink-dl/frontend/src/*`, `@stats/*` = `plugins/yoink-stats/frontend/src/*`, `@insight/*` = `plugins/yoink-insight/frontend/src/*`.

**Alias resolution:** `vite.config.ts` `resolve.alias` is the sole runtime resolver (no `vite-tsconfig-paths` plugin). `tsconfig.json` paths remain in sync for tsc/editor only.

**Page naming:** all page files use PascalCase (`AdminGroupsPage.tsx`, `GroupPage.tsx`, etc.). Index barrels (`index.ts`) export the public surface of each feature directory.

**Logic hooks:** heavy pages extract state/logic into co-located `usePage.ts` hooks (`useAdminGroups`, `useAdminCookies`) keeping JSX thin.

**API layer:** all `apiClient` calls go through typed modules in `lib/api/` and plugin `api/` directories. Pages and hooks import from those modules, not from `apiClient` directly.

## Database migrations

Single Alembic chain covering core and all plugins:

| Migration | Description |
|---|---|
| 0001 | users, groups, thread_policies, bot_settings, events |
| 0002 | dl settings, file cache, download log, rate limits, cookies |
| 0003 | message log, stats tables |
| 0004 | stats tsvector full-text index |
| 0005 | inline storage settings for groups |
| 0006 | user is_premium flag |
| 0007 | dl DM topic thread ID |
| 0008 | M2M API keys |
| 0009 | insight_access table |
| 0010 | file_cache multi-file (file_ids JSON) |
| 0011 | gallery_zip flag in download_log |
| 0012 | unified user_permissions table |
| 0013 | insight_user_settings table |
| 0014 | clip_start, clip_end, group_title in download_log |
| 0015 | cookies.inherited flag |
| 0016 | music download_log fields (user_id, group_id, thread_id) |
| 0017 | stats ranked list and period fields |
| 0018 | users.photo_url |
| 0019 | file_cache.cache_key String(80) |
| 0020 | cookies.is_pool flag + index |
| 0021 | cookies.label |
| 0022 | cookies partial unique index (personal) |
| 0023 | cookies.avatar_url |
| 0024 | cookies.content_hash + index |
| 0025 | cookies.session_key + index |
| 0026 | dl_user_settings.use_pool_cookies |
| 0027 | groups.photo_url |
| 0028 | stats_reactions table |
| 0029 | stats_group_members table |
| 0030 | stats_chat_admins table |

## Custom Bot API server

Built from [tdlight-telegram-bot-api](https://github.com/tdlight-team/tdlight-telegram-bot-api) with `docker/patches/tdlight-forum-extras.patch`. Adds methods for forum topics, message viewers, chat history, etc.

```bash
just build tg   # ~10 min first build, cached thereafter
```

Current server version: **9.5**.

## User-mode session

The bot API server runs with `--allow-users`, enabling both bot and user accounts. The user session unlocks API methods unavailable to bots (forum topic listing, chat history, etc). It is strictly optional - all core functionality works without it; user-session features are only shown in the UI when `GET /threads/status` returns `available: true`.

```bash
just tg login +79001234567
just tg status
just tg logout
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
just backup restore list  # list available backups
just backup restore       # restore latest
just backup up        # start cron (daily at 03:00)
```

Retention: 7 daily + 4 weekly.

## Tests

```bash
just test                        # core tests
cd plugins/yoink-dl && just test
cd plugins/yoink-stats && just test
```
