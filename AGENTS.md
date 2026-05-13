# AGENTS.md - yoink-core

## Critical Rules
- Never inline SQL in Python - use `.sql` files loaded via `load_sql(base, name)` from `yoink.core.db.query`
- Never add an i18n key to only one locale - always update both `en.yml` and `ru.yml` together
- Never add a new `DownloadError` for a known yt-dlp error pattern - use or create a specific `BotError` subclass
- Never edit files under `frontend/src/components/ui/` (shadcn managed)
- Never call `apiClient` directly from pages - use typed modules in `lib/api/` or plugin `api/` dirs
- Never add an Alembic migration without a matching model change (and vice versa)
- Never add `proxy_url` as a compose interpolation variable inside `.env` itself - compose self-reference doesn't work

## Tech Stack
- Python 3.12, uv, FastAPI, SQLAlchemy async 2.0, Alembic, pydantic-settings, python-telegram-bot
- React 18, TypeScript, Vite, Tailwind CSS, shadcn/ui
- PostgreSQL 16, Docker Compose, yt-dlp, gallery-dl, ffmpeg

## Commands
- Build: `just build [yoink|frontend|tg|backup|all]`
- Up/down: `just up` / `just down` / `just restart <service>` / `just logs [service]`
- Tests: `just test [path]` (core); `cd plugins/yoink-dl && just test`
- Migrations: `just migrate [up|down|create "msg"|current|history]`

## Structure
- `src/yoink/` - core: RBAC, auth, DB, bot infra, i18n, API
- `plugins/yoink-dl/` - yt-dlp downloader (main plugin)
- `plugins/yoink-stats/`, `yoink-music/`, `yoink-insight/` - additional plugins
- `frontend/` - React SPA; plugin frontends live in `plugins/*/frontend/`
- `db/migrations/versions/` - single Alembic chain for core + all plugins
- `.env` - single source of truth for all services; all compose services read it via `env_file: .env`

## Python Conventions
- User-facing errors: subclass `BotError` in `utils/errors.py` + i18n key in both locale files
- yt-dlp error classification: check `str(e).lower()` for known substrings, raise specific `BotError`; fallback is raw `DownloadError`. `_is_retryable()` in `url/pipeline/helpers.py`: `BotError` subclasses are NOT retried, plain `DownloadError` IS
- DB models: `DeclarativeBase` + `AuditMixin`/`SoftDeleteMixin` from `yoink.core.db.base`
- Config: `BaseSettings` from pydantic-settings; env vars lowercase in `.env`; `proxy_url` = single proxy string, `proxy_urls` = JSON list for round-robin
- Plugin registration: `[project.entry-points."yoink.plugins"]` in the plugin's `pyproject.toml`

## RBAC
- Role hierarchy: `banned < restricted < user < moderator < admin < owner`
- New gated feature: add `FeatureSpec` in plugin's `get_features()` + `CommandSpec(required_feature=...)` on the command

## Frontend Conventions
- Path aliases: `@core/*` = `frontend/src/*`, `@ui` = shadcn components, `@app` = app-level components; `@dl/*`, `@stats/*`, `@insight/*` for plugin frontends
- Alias config lives in `vite.config.ts` only - keep `tsconfig.json` in sync manually (no vite-tsconfig-paths)
- Page files: PascalCase (`AdminGroupsPage.tsx`); heavy pages extract logic into co-located `usePageName.ts` hooks
