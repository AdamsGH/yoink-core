# yoink-inbox

Link-collection inbox for the yoink bot. Accepts URLs (articles, repos, anything) from the Telegram bot, REST API, or web UI; enriches metadata via `yoink-insight.services.fetch`; categorises with an LLM via `yoink-insight.services.byok`; for GitHub URLs maintains a viewer of the user's starred repos with folder-based organisation, drag-and-drop, and optional write-back (star/unstar).

## Features

- **Inbox items** (`inbox:ingest`): save any URL via bot, API, or web UI; metadata fetched and stored; LLM classification optional (`inbox:classify`).
- **Categories**: per-user item categories with optional team sharing.
- **GitHub Stars** (`inbox:gh_sync`): periodic sync of the user's GitHub stars; full-text search, language filter, topic badges.
- **Folder organisation**: drag starred repos into named folders via the web UI (dnd-kit). Folders support create, rename, delete, and item badge counts.
- **GitHub write access** (`inbox:gh_write`): star and unstar repos directly from the UI via a separate OAuth App (`public_repo` scope). Users connect via device-flow in Insight Settings; token stored in `insight_user_settings.github_token_public_repo`.
- **Rules engine** (`inbox_rules`): trigger + conditions + actions stored as JSONB, evaluated at classify and gh_sync tail.
- **Teams** (`inbox_teams`): share access across users, independent of Telegram groups.

## Design notes

- Plugin name: `inbox`. Route prefix `/api/v1/inbox/`. Feature namespace `inbox:*`.
- Categories are per-user with opt-in sharing to an inbox-specific team (`inbox_teams`), independent of Telegram chat groups.
- GitHub OAuth: read-only stars work on insight's existing `read:user` scope. Star/unstar from UI is gated by `inbox:gh_write`; requires `public_repo` token from a separate OAuth App (configured in `.env` as `github_oauth_public_repo_client_id` / `_secret`); token stored in `insight_user_settings.github_token_public_repo` via yoink-insight's device-flow endpoints.
- Background work runs on ARQ (Redis-backed) with separate queues per stage. PTB JobQueue is only used to schedule the periodic GH star sync.
- Rules engine (`inbox_rules`) is part of the first release: trigger + conditions + actions stored as JSONB, evaluated at the tail of classify and gh_sync.

## Feature flags

| Feature | `default_min_role` | Description |
|---|---|---|
| `inbox:ingest` | `user` | Save items; view inbox |
| `inbox:classify` | none | LLM auto-classification (requires explicit grant) |
| `inbox:share` | `user` | Share items |
| `inbox:gh_sync` | none | Sync and view GitHub stars |
| `inbox:gh_organise` | none | Folder CRUD and drag-and-drop organisation |
| `inbox:gh_write` | none | Star / unstar repos on GitHub |
| `inbox:admin` | `admin` | Admin panel |

`none` = explicit grant required; owner always passes.

## API endpoints

Mounted at `/api/v1/inbox/`. Auth: JWT Bearer token.

| Method | Path | Feature | Description |
|---|---|---|---|
| GET | /items | `ingest` | List items (cursor-paginated) |
| POST | /items | `ingest` | Create item |
| GET | /items/{id} | `ingest` | Get item |
| DELETE | /items/{id} | `ingest` | Archive (soft delete) |
| GET | /categories | `ingest` | List categories |
| POST | /categories | `ingest` | Create category |
| GET | /categories/{slug} | `ingest` | Get category with items |
| PUT | /categories/{id} | `ingest` | Update category |
| DELETE | /categories/{id} | `ingest` | Delete category |
| GET | /gh_stars | `gh_sync` | List stars (cursor-paginated; filter by folder_id, language, search) |
| POST | /gh_stars/sync | `gh_sync` | Enqueue GH star sync |
| GET | /folders | `gh_organise` | List folders |
| POST | /folders | `gh_organise` | Create folder |
| GET | /folders/{id} | `gh_organise` | Get folder |
| PUT | /folders/{id} | `gh_organise` | Update folder |
| DELETE | /folders/{id} | `gh_organise` | Delete folder |
| POST | /folders/{id}/stars/{sid} | `gh_organise` | Add star to folder |
| DELETE | /folders/{id}/stars/{sid} | `gh_organise` | Remove star from folder |
| PUT | /gh_stars/{id}/star | `gh_write` | Star repo on GitHub |
| DELETE | /gh_stars/{id}/star | `gh_write` | Unstar repo on GitHub |
| GET | /rules | `ingest` | List rules |
| POST | /rules | `ingest` | Create rule |
| PUT | /rules/{id} | `ingest` | Update rule |
| DELETE | /rules/{id} | `ingest` | Delete rule |
| POST | /rules/test | `ingest` | Dry-run rules against a URL |
| GET | /teams | `ingest` | List teams |
| POST | /teams | `ingest` | Create team |
| PUT | /teams/{id} | `ingest` | Update team |
| DELETE | /teams/{id} | `ingest` | Delete team |
| POST | /teams/{id}/members/{uid} | `admin` | Add member |
| PATCH | /teams/{id}/members/{uid} | `admin` | Update member role |
| DELETE | /teams/{id}/members/{uid} | `admin` | Remove member |

## Configuration

| Variable | Required | Default | Description |
|---|---|---|---|
| `github_oauth_public_repo_client_id` | no | - | GitHub OAuth App client_id for `public_repo` write scope device-flow |
| `github_oauth_public_repo_client_secret` | no | - | Corresponding client secret |

Register a separate GitHub OAuth App at `https://github.com/settings/developers` with callback URL unused (device flow). The `read:user` App for star-read is owned by yoink-insight and configured separately.

## Database

| Migration | Description |
|---|---|
| 0047 | `inbox_items`, `inbox_categories`, `inbox_item_categories` tables |
| 0048 | `inbox_gh_stars`, `inbox_gh_folders`, `inbox_gh_star_folders`, `inbox_rules`, `inbox_teams`, `inbox_team_members` tables; `inbox_user_sync_state` |
| 0049 | `insight_user_settings.github_token_public_repo` column (write token) |
