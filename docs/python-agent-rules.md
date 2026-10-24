# Python and database changes

- Store SQL in `.sql` files. Load it with `load_sql(base, name)` from `yoink.core.db.query`.
- Import SQLAlchemy annotation types at runtime, including `Mapped[datetime]`. SQLAlchemy evaluates annotations during class construction. Use `# noqa: TC003` where needed.
- For known yt-dlp error patterns, use or create a specific `BotError` subclass.
- Pair Alembic migrations with matching model changes.
- Keep Alembic migration annotations immediate. Omit `from __future__ import annotations`; it creates a separate import block that triggers ruff I001.
- Limit Alembic revision IDs to 32 characters. `alembic_version.version_num` is `VARCHAR(32)`; longer IDs fail at migration time.
- In migrations, place `from alembic import op` before `import sqlalchemy as sa`. Keep this order even where per-file ignores exempt migrations.
- Read service and router configuration through `CoreSettings`, `DownloaderConfig` or `InsightConfig`, not direct environment lookups. Keep `.env.example` authoritative.
- In async contexts, use `asyncio.to_thread`, `aiofiles` or `httpx.AsyncClient` for blocking I/O. This includes `time.sleep`, `httpx.get`, `f.open("rb")` and `YouTubeTranscriptApi.fetch`.
- Put operational timeouts in the owning plugin config. Pass them through function parameters rather than module globals or `configure_timeouts()`.
- Do not interpolate `proxy_url` from itself inside `.env`; Compose self-reference does not work.

## Python conventions

- User-facing errors: subclass `BotError` in `utils/errors.py` + i18n key in both locale files
- yt-dlp error classification: check `str(e).lower()` for known substrings, raise specific `BotError`; fallback is raw `DownloadError`. `_is_retryable()` in `url/pipeline/helpers.py`: `BotError` subclasses are NOT retried, plain `DownloadError` IS
- DB models: `DeclarativeBase` + `AuditMixin`/`SoftDeleteMixin` from `yoink.core.db.base`
- Config: `BaseSettings` from pydantic-settings; env vars lowercase in `.env`; `proxy_url` = single proxy string, `proxy_urls` = JSON list for round-robin
- Plugin registration: `[project.entry-points."yoink.plugins"]` in the plugin's `pyproject.toml`
- Logging: `logger.exception(...)` for unexpected errors (full traceback); `logger.debug/warning` for expected fallback chains (e.g. "platform A failed -> try platform B")

## RBAC

- Role hierarchy: `banned < restricted < user < moderator < admin < owner`
- New gated feature: add `FeatureSpec` in plugin's `get_features()` + `CommandSpec(required_feature=...)` on the command; plugins register extra grant sources via `register_feature_provider` (from `yoink.core.auth.effective_features`) in `setup(ctx)` - `EffectiveFeatureResolver` is the single source of truth for all grant checks (see kb:yoink:effective-feature-resolver)
- After any grant mutation (PUT/DELETE `/byok/me`, `PATCH /config/byok`, POST/DELETE `/access/{uid}`, `PATCH /users/{id}`): call `refresh_user_commands` - all four grant-checking sites go through `EffectiveFeatureResolver`; skipping this leaves command lists stale silently
