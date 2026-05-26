# AGENTS.md - yoink-core

## CRITICAL: never run uv / python / pip / pytest on the host
All Python work happens inside the docker containers. `uv.lock` is gitignored AND must stay un-tracked; running `uv sync`, `uv run`, `uv pip install`, `pip install`, `pytest`, or any `python -c '...'` from the host re-creates `.venv/`, regenerates `uv.lock`, and worst of all bakes host-side resolver state into commits. `gitignore` does NOT protect a file once it is already tracked, and `git commit paths=[..., 'uv.lock']` will happily re-add it. Use `just build`, `just up`, `just test`, `just migrate` (or `docker compose run --rm yoink ...`) for every Python command; if a typed verification step does not exist for what you need, add it to the justfile rather than reaching for host `uv`. If `uv.lock` ever shows up in `git status`, `git rm --cached uv.lock` immediately and commit the deletion before doing anything else.

## Critical Rules
- Never inline SQL in Python - use `.sql` files loaded via `load_sql(base, name)` from `yoink.core.db.query`
- Never add an i18n key to only one locale - always update both `en.yml` and `ru.yml` together
- Never add a new `DownloadError` for a known yt-dlp error pattern - use or create a specific `BotError` subclass
- Never edit files under `frontend/src/components/ui/` (shadcn managed)
- Never inline empty-state divs - use `EmptyState` from `@app` instead of `<div className="flex justify-center py-12 text-sm text-muted-foreground">`
- Never inline divide-y lists - use `DividedList` from `@ui` instead of `<div className="divide-y divide-border px-3 py-1">`
- Never inline skeleton loops - use `SkeletonList` from `@ui` instead of `Array.from({ length: N }).map((_, i) => ...)`
- Never call `apiClient` directly from pages - use typed modules in `lib/api/` or plugin `api/` dirs
- Never add an Alembic migration without a matching model change (and vice versa)
- Never add `proxy_url` as a compose interpolation variable inside `.env` itself - compose self-reference doesn't work
- Never read env vars via `os.getenv` / `os.environ.get` inside services or routers - they must flow through `CoreSettings` / `DownloaderConfig` / `InsightConfig` so .env.example stays authoritative
- Never do sync I/O (`time.sleep`, `httpx.get`, `f.open("rb")`, `YouTubeTranscriptApi.fetch`) in async contexts - wrap in `asyncio.to_thread` or use `aiofiles` / `httpx.AsyncClient`
- Never hardcode operational timeouts in plugin services - add a field to the plugin's `DownloaderConfig`/`InsightConfig`/etc. and thread it through function signatures (no module-level globals or `configure_timeouts()` patterns)

## Tech Stack
- Python 3.12, uv, FastAPI, SQLAlchemy async 2.0, Alembic, pydantic-settings, python-telegram-bot
- React 18, TypeScript, Vite, Tailwind CSS, shadcn/ui, react-i18next (en/ru)
- PostgreSQL 16, Docker Compose, yt-dlp, gallery-dl, ffmpeg
- httpx (async), tenacity (retry), aiofiles

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
- Logging: `logger.exception(...)` for unexpected errors (full traceback); `logger.debug/warning` for expected fallback chains (e.g. "platform A failed -> try platform B")
- Pipeline split: `plugins/yoink-dl/src/yoink_dl/url/pipeline/` follows the named-phase pattern - orchestrator (`run.py`) + `download_phase.py` / `upload_phase.py` / `cache.py` / `guards.py` / `helpers.py`. Error handler lives in `helpers.handle_download_error`; metrics counters (`downloads_ok` / `downloads_error` / `download_duration_seconds`) stay in the orchestrator
- Cookie parsing: pure netscape-format helpers (`validate_netscape`, `extract_account_label`, `_parse_netscape_cookies`, ...) live in `services/cookies_netscape.py`; `services/cookies.py` keeps stateful `CookieManager` + `_CookieCycle` and re-exports the pure helpers for back-compat

## RBAC
- Role hierarchy: `banned < restricted < user < moderator < admin < owner`
- New gated feature: add `FeatureSpec` in plugin's `get_features()` + `CommandSpec(required_feature=...)` on the command

## Frontend Conventions
- Path aliases: `@core/*` = `frontend/src/*`, `@ui` = shadcn components, `@app` = app-level components, `@core/components/form` = react-hook-form controllers, `@core/components/charts` = chart wrappers; `@dl/*`, `@stats/*`, `@insight/*` for plugin frontends
- Alias config lives in `vite.config.ts` only - keep `tsconfig.json` in sync manually (no vite-tsconfig-paths)
- Page files: PascalCase (`AdminGroupsPage.tsx`); heavy pages extract logic into co-located `usePageName.ts` hooks (pattern: `AdminUsersPage` -> `useAdminUsers.ts`, `AdminGroupsPage` -> `useAdminGroups.ts`)
- Large pages with multi-tab drawers split sub-tabs into co-located components (pattern: `UserDrawer.tsx` shell + `BanDatePicker.tsx` + `UserDrawerStatsTab.tsx` + `UserDrawerPermissionsTab.tsx` next to each other, NO barrel `index.ts` re-export)
- Date formatting: use `formatDate` / `formatDateMonth` / `formatDateDay` from `@core/lib/utils` - the project does NOT depend on `date-fns`
- Dev-only diagnostics: gate `console.error` / `console.warn` with `if (import.meta.env.DEV)` so production builds stay quiet
- User confirmations: shadcn `AlertDialog` instead of native `confirm()` / `alert()`, except admin-only surfaces where the native dialog is acceptable (still i18n the text)

### Shared UI components
- `EmptyState` (`@app`) - centered empty state message inside a card
- `CompactCardHeader` (`@app`) - `px-4 py-3` card header with title + optional actions; skip when the header has extra content below the title row
- `DividedList` (`@ui`) - `divide-y divide-border px-3 py-1` wrapper
- `SkeletonList` (`@ui`) - skeleton row repeater: `<SkeletonList count={N}>{(i) => <Row key={i} />}</SkeletonList>`
- `IconButton` (`@ui`) - ghost icon button with Tooltip; `variant` prop for outline/destructive variants
- `DialogActions` (`@ui`) - `DialogFooter` with `flex-row gap-2 sm:space-x-0`
- `ControlledSelect<T>`, `ControlledSwitch<T>` (`@core/components/form`) - react-hook-form controllers for Select and Switch
- `ChartSkeleton`, `SectionSkeleton` (`@core/components/charts`) - loading skeletons for chart sections
- `MiniBarChart` (`@core/components/charts`) - recharts BarChart wrapper (expects `date` key for XAxis)
- `HorizontalBars` (`@core/components/charts`) - ranked bar list with `colors[]` array
- `RankedList<T>` (`@stats/components`) - generic typed ranked list with `bg-primary/60` bars
