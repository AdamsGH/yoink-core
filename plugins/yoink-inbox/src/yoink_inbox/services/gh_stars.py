"""GitHub starred-repos snapshot sync.

One public entry point: `run_sync(session_factory, user_id)`. The worker
calls it from `sync_user_stars`; the bot calls it via `/stars_sync`.

Algorithm:

1. Resolve the user's GH token from `insight_user_settings` (the insight
   plugin owns the OAuth flow); no token -> short-circuit with
   `last_status='no_token'`.
2. Look up the cached ETag in `inbox_gh_sync_state` and send it as
   `If-None-Match`. GitHub returns 304 if nothing changed; we update
   `last_synced_at` and bail.
3. Otherwise paginate `/user/starred?per_page=N` until exhaustion. Each
   row is upserted into `inbox_gh_stars` keyed on `(user_id, gh_repo_id)`;
   pre-existing `ai_labels` / `ai_summary` are preserved (we only touch
   the snapshot fields). `last_synced_at` is bumped to NOW so the cleanup
   step can spot unstarred repos.
4. After full pagination, delete rows with `last_synced_at < sync_started`;
   those are repos the user unstarred since the last run. The
   `inbox_item_to_star` bridge is `ON DELETE CASCADE`, so dangling links
   drop with them.
5. Persist new ETag from the FIRST page (GitHub's ETag identifies the
   logical resource, not a page; the first page's value is the right one
   for `If-None-Match`).

The function is idempotent and safe to invoke concurrently for the same
user (the worker semaphore caps concurrency to 1 per user via
`max_jobs`); the outer transaction protects the snapshot swap.

Errors are classified into stable `last_status` values:
  no_token, ok, not_modified, auth_failed, rate_limited, error.
"""
from __future__ import annotations

import logging
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any

import httpx
from sqlalchemy import delete, select
from sqlalchemy.dialects.postgresql import insert as pg_insert

from yoink_inbox.config import InboxConfig
from yoink_inbox.storage.models import (
    InboxGhStar,
    InboxGhSyncState,
)

if TYPE_CHECKING:
    from sqlalchemy.ext.asyncio import async_sessionmaker

logger = logging.getLogger(__name__)


_GITHUB_API = "https://api.github.com"
_USER_AGENT = "yoink-inbox/0.1"
# Accept header that asks GitHub to include the starred_at timestamp on each
# repo (the default response shape omits it).
_ACCEPT_STARRED_AT = "application/vnd.github.star+json"

# Link header looks like:
#   <https://api.github.com/user/starred?per_page=100&page=2>; rel="next", ...
# We only need the next-page URL; presence is the only thing the parser
# returns.
_LINK_NEXT_RE = re.compile(r'<([^>]+)>;\s*rel="next"')


@dataclass(slots=True)
class SyncResult:
    status: str  # "ok" | "not_modified" | "no_token" | "auth_failed" | "rate_limited" | "error"
    stars_count: int
    removed: int
    error: str | None = None


def _parse_next_link(link_header: str | None) -> str | None:
    if not link_header:
        return None
    m = _LINK_NEXT_RE.search(link_header)
    return m.group(1) if m else None


def _parse_starred_at(value: Any) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


async def _resolve_token(session_factory: "async_sessionmaker", user_id: int) -> str | None:
    """Pull the GitHub OAuth token from insight's user settings.

    Returns None if the user never linked GitHub or revoked the link.
    """
    # Lazy import: avoids a hard circular at module import time and keeps
    # insight an optional dep for users who do not install it.
    from yoink_insight.storage.repos import InsightUserSettingsRepo

    repo = InsightUserSettingsRepo(session_factory)
    return await repo.get_github_token(user_id)


async def _fetch_page(
    client: httpx.AsyncClient,
    url: str,
    token: str,
    *,
    etag: str | None,
) -> httpx.Response:
    headers = {
        "Authorization": f"token {token}",
        "Accept": _ACCEPT_STARRED_AT,
        "User-Agent": _USER_AGENT,
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if etag is not None:
        headers["If-None-Match"] = etag
    return await client.get(url, headers=headers)


async def _upsert_star(
    session,
    user_id: int,
    repo_payload: dict,
    starred_at: datetime | None,
    sync_started: datetime,
    can_unstar: bool,
) -> None:
    """Upsert one starred repo, preserving AI fields on existing rows."""
    values = {
        "user_id": user_id,
        "gh_repo_id": int(repo_payload["id"]),
        "full_name": repo_payload.get("full_name") or "",
        "owner_login": (repo_payload.get("owner") or {}).get("login") or "",
        "owner_avatar_url": (repo_payload.get("owner") or {}).get("avatar_url"),
        "description": repo_payload.get("description"),
        "language": repo_payload.get("language"),
        "topics": repo_payload.get("topics") or None,
        "stargazers_count": int(repo_payload.get("stargazers_count") or 0),
        "html_url": repo_payload.get("html_url") or "",
        "homepage": repo_payload.get("homepage") or None,
        "archived": bool(repo_payload.get("archived")),
        "fork": bool(repo_payload.get("fork")),
        "starred_at": starred_at,
        "updated_at": _parse_starred_at(repo_payload.get("updated_at")),
        "can_unstar": can_unstar,
        "last_synced_at": sync_started,
    }
    stmt = pg_insert(InboxGhStar).values(**values)
    # On conflict: refresh snapshot fields, leave ai_labels / ai_summary alone.
    stmt = stmt.on_conflict_do_update(
        constraint="uq_inbox_gh_stars_user_repo",
        set_={
            "full_name": stmt.excluded.full_name,
            "owner_login": stmt.excluded.owner_login,
            "owner_avatar_url": stmt.excluded.owner_avatar_url,
            "description": stmt.excluded.description,
            "language": stmt.excluded.language,
            "topics": stmt.excluded.topics,
            "stargazers_count": stmt.excluded.stargazers_count,
            "html_url": stmt.excluded.html_url,
            "homepage": stmt.excluded.homepage,
            "archived": stmt.excluded.archived,
            "fork": stmt.excluded.fork,
            "starred_at": stmt.excluded.starred_at,
            "updated_at": stmt.excluded.updated_at,
            "can_unstar": stmt.excluded.can_unstar,
            "last_synced_at": stmt.excluded.last_synced_at,
        },
    )
    await session.execute(stmt)


async def _write_sync_state(
    session,
    user_id: int,
    *,
    etag: str | None,
    status: str,
    error: str | None,
    stars_count: int,
    last_synced_at: datetime,
) -> None:
    stmt = pg_insert(InboxGhSyncState).values(
        user_id=user_id,
        etag=etag,
        last_synced_at=last_synced_at,
        last_status=status,
        last_error=error,
        stars_count=stars_count,
    )
    stmt = stmt.on_conflict_do_update(
        index_elements=["user_id"],
        set_={
            "etag": stmt.excluded.etag,
            "last_synced_at": stmt.excluded.last_synced_at,
            "last_status": stmt.excluded.last_status,
            "last_error": stmt.excluded.last_error,
            "stars_count": stmt.excluded.stars_count,
        },
    )
    await session.execute(stmt)


async def run_sync(
    session_factory: "async_sessionmaker", user_id: int
) -> SyncResult:
    """Snapshot-sync this user's starred repos.

    Always returns a SyncResult; never raises. Status codes are the source
    of truth for the caller. Internally writes to inbox_gh_stars and
    inbox_gh_sync_state in one transaction per page so a network hiccup
    mid-pagination still leaves the partial state visible (the cleanup at
    the end is the only destructive step).
    """
    cfg = InboxConfig()

    token = await _resolve_token(session_factory, user_id)
    if token is None:
        async with session_factory() as s:
            await _write_sync_state(
                s,
                user_id,
                etag=None,
                status="no_token",
                error=None,
                stars_count=0,
                last_synced_at=datetime.now(timezone.utc),
            )
            await s.commit()
        return SyncResult(status="no_token", stars_count=0, removed=0)

    # Read prior ETag so we can short-circuit on 304.
    async with session_factory() as s:
        prior_etag = await s.scalar(
            select(InboxGhSyncState.etag).where(InboxGhSyncState.user_id == user_id)
        )

    sync_started = datetime.now(timezone.utc)
    page_size = cfg.inbox_gh_sync_page_size
    next_url: str | None = (
        f"{_GITHUB_API}/user/starred?per_page={page_size}&sort=created&direction=desc"
    )
    first_page = True
    new_etag: str | None = None
    total_seen = 0

    # can_unstar follows whether the token has write scope; we read X-OAuth-Scopes
    # off the first response.
    can_unstar = False

    async with httpx.AsyncClient(timeout=30.0) as client:
        while next_url is not None:
            try:
                resp = await _fetch_page(client, next_url, token, etag=prior_etag if first_page else None)
            except httpx.HTTPError as exc:
                logger.warning("inbox.gh_stars transport failed user_id=%s err=%s", user_id, exc)
                async with session_factory() as s:
                    await _write_sync_state(
                        s,
                        user_id,
                        etag=prior_etag,
                        status="error",
                        error=str(exc)[:500],
                        stars_count=total_seen,
                        last_synced_at=sync_started,
                    )
                    await s.commit()
                return SyncResult(status="error", stars_count=total_seen, removed=0, error=str(exc))

            if first_page:
                if resp.status_code == 304:
                    # ETag still valid, no work.
                    async with session_factory() as s:
                        prior_count = await s.scalar(
                            select(InboxGhSyncState.stars_count).where(
                                InboxGhSyncState.user_id == user_id
                            )
                        ) or 0
                        await _write_sync_state(
                            s,
                            user_id,
                            etag=prior_etag,
                            status="not_modified",
                            error=None,
                            stars_count=prior_count,
                            last_synced_at=sync_started,
                        )
                        await s.commit()
                    return SyncResult(status="not_modified", stars_count=prior_count, removed=0)
                if resp.status_code == 401:
                    async with session_factory() as s:
                        await _write_sync_state(
                            s,
                            user_id,
                            etag=None,  # nuke cached etag; token may have been rotated
                            status="auth_failed",
                            error="GitHub returned 401",
                            stars_count=0,
                            last_synced_at=sync_started,
                        )
                        await s.commit()
                    return SyncResult(status="auth_failed", stars_count=0, removed=0)
                if resp.status_code == 403 and "rate limit" in (resp.text or "").lower():
                    async with session_factory() as s:
                        await _write_sync_state(
                            s,
                            user_id,
                            etag=prior_etag,
                            status="rate_limited",
                            error="GitHub rate limit",
                            stars_count=0,
                            last_synced_at=sync_started,
                        )
                        await s.commit()
                    return SyncResult(status="rate_limited", stars_count=0, removed=0)
                # Capture etag + scopes from the first 2xx page.
                new_etag = resp.headers.get("ETag")
                scopes = resp.headers.get("X-OAuth-Scopes", "")
                can_unstar = "public_repo" in scopes or scopes.strip() == "repo"
                first_page = False

            if resp.status_code >= 400:
                logger.warning(
                    "inbox.gh_stars unexpected status user_id=%s code=%s",
                    user_id, resp.status_code,
                )
                async with session_factory() as s:
                    await _write_sync_state(
                        s,
                        user_id,
                        etag=prior_etag,
                        status="error",
                        error=f"HTTP {resp.status_code}",
                        stars_count=total_seen,
                        last_synced_at=sync_started,
                    )
                    await s.commit()
                return SyncResult(
                    status="error", stars_count=total_seen, removed=0,
                    error=f"HTTP {resp.status_code}",
                )

            try:
                rows = resp.json()
            except ValueError:
                rows = []
            if not isinstance(rows, list):
                rows = []

            async with session_factory() as s:
                for entry in rows:
                    if not isinstance(entry, dict):
                        continue
                    # With Accept=star+json each row is {"starred_at": ..., "repo": {...}}.
                    repo_payload = entry.get("repo") if "repo" in entry else entry
                    if not isinstance(repo_payload, dict) or repo_payload.get("id") is None:
                        continue
                    starred_at = _parse_starred_at(entry.get("starred_at"))
                    await _upsert_star(
                        s, user_id, repo_payload, starred_at, sync_started, can_unstar,
                    )
                    total_seen += 1
                await s.commit()

            next_url = _parse_next_link(resp.headers.get("Link"))

    # Cleanup: anything not refreshed in this run is no longer starred.
    async with session_factory() as s:
        deleted = await s.execute(
            delete(InboxGhStar).where(
                InboxGhStar.user_id == user_id,
                InboxGhStar.last_synced_at < sync_started,
            )
        )
        removed = deleted.rowcount or 0
        await _write_sync_state(
            s,
            user_id,
            etag=new_etag,
            status="ok",
            error=None,
            stars_count=total_seen,
            last_synced_at=sync_started,
        )
        await s.commit()

    logger.info(
        "inbox.gh_stars sync done user_id=%s stars=%d removed=%d etag=%s",
        user_id, total_seen, removed, new_etag,
    )
    return SyncResult(status="ok", stars_count=total_seen, removed=removed)
