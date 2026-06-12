# yoink-inbox

Link-collection inbox for the yoink bot. Accepts URLs (articles, repos, anything) from the Telegram bot, REST API, or web UI; enriches metadata via `yoink-insight.services.fetch`; categorises with an LLM via `yoink-insight.services.byok`; for GitHub URLs maintains a viewer of the user's starred repos and folder-based organisation.

## Design notes

- Plugin name: `inbox`. Route prefix `/api/v1/inbox/`. Feature namespace `inbox:*`.
- Categories are per-user with opt-in sharing to an inbox-specific team (`inbox_teams`), independent of Telegram chat groups.
- GitHub OAuth: read-only stars work on insight's existing `read:user` scope. Star/unstar from UI is gated by an opt-in `public_repo` upgrade exchanged through a separate OAuth App; token stored in `insight_user_settings.github_token_public_repo`.
- Background work runs on ARQ (Redis-backed) with separate queues per stage. PTB JobQueue is only used to schedule the periodic GH star sync.
- Rules engine (`inbox_rules`) is part of the first release, not deferred: trigger + conditions + actions stored as JSONB, evaluated at the tail of classify and gh_sync.

See `~/.pi/agent/` TODO-c66b1c02 for the full plan.
