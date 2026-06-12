"""GitHub write operations (star / unstar) for yoink-inbox.

Requires the user to have a public_repo token stored in
insight_user_settings.github_token_public_repo (obtained via the
/insight/github/upgrade-scope device flow).

Raises PermissionError when the token is absent so callers can surface
a clear 403 rather than a generic 500.
"""
from __future__ import annotations

import logging

import httpx

logger = logging.getLogger(__name__)

_GH_STAR_URL = "https://api.github.com/user/starred/{owner}/{repo}"
_USER_AGENT = "yoink-inbox-gh-write/1.0"


async def _get_public_repo_token(session_factory, user_id: int) -> str:
    """Return the public_repo token or raise PermissionError."""
    try:
        from yoink_insight.storage.repos import InsightUserSettingsRepo
        token = await InsightUserSettingsRepo(session_factory).get_github_token_public_repo(user_id)
    except ImportError:
        raise PermissionError("yoink-insight not installed; cannot access GitHub write token")
    if not token:
        raise PermissionError("No public_repo token. Connect GitHub write access in settings first.")
    return token


async def star_repo(session_factory, user_id: int, owner: str, repo: str) -> None:
    """Star a repository on behalf of the user."""
    token = await _get_public_repo_token(session_factory, user_id)
    url = _GH_STAR_URL.format(owner=owner, repo=repo)
    async with httpx.AsyncClient(timeout=15) as client:
        resp = await client.put(
            url,
            headers={
                "authorization": f"token {token}",
                "accept": "application/vnd.github+json",
                "content-length": "0",
                "user-agent": _USER_AGENT,
            },
        )
    if resp.status_code == 401:
        raise PermissionError("GitHub token rejected (401). Re-connect write access.")
    if resp.status_code not in (204, 304):
        resp.raise_for_status()
    logger.info("inbox.gh_write: starred %s/%s for user_id=%s", owner, repo, user_id)


async def unstar_repo(session_factory, user_id: int, owner: str, repo: str) -> None:
    """Unstar a repository on behalf of the user."""
    token = await _get_public_repo_token(session_factory, user_id)
    url = _GH_STAR_URL.format(owner=owner, repo=repo)
    async with httpx.AsyncClient(timeout=15) as client:
        resp = await client.delete(
            url,
            headers={
                "authorization": f"token {token}",
                "accept": "application/vnd.github+json",
                "user-agent": _USER_AGENT,
            },
        )
    if resp.status_code == 401:
        raise PermissionError("GitHub token rejected (401). Re-connect write access.")
    if resp.status_code not in (204, 304):
        resp.raise_for_status()
    logger.info("inbox.gh_write: unstarred %s/%s for user_id=%s", owner, repo, user_id)
