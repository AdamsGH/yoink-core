"""GitHub write operations (star / unstar) for yoink-inbox.

Requires the user to have a public_repo token stored in
insight_user_settings.github_token_public_repo (obtained via the
/insight/github/upgrade-scope device flow).

Raises PermissionError when the token is absent so callers can surface
a clear 403 rather than a generic 500.
"""
from __future__ import annotations

import logging
import os

import httpx

logger = logging.getLogger(__name__)

_GH_STAR_URL = "https://api.github.com/user/starred/{owner}/{repo}"
_USER_AGENT = "yoink-inbox-gh-write/1.0"
_PROXY = os.environ.get("proxy_url") or os.environ.get("PROXY_URL") or None


def _make_client(**kwargs) -> httpx.AsyncClient:
    if _PROXY:
        kwargs.setdefault("proxy", _PROXY)
    return httpx.AsyncClient(**kwargs)


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
    async with _make_client(timeout=15) as client:
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
    async with _make_client(timeout=15) as client:
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


async def add_to_gh_list(
    session_factory,
    user_id: int,
    repo_node_id: str,
    list_id: str,
) -> None:
    """Add a repo to a GitHub List (safe add, preserves other memberships)."""
    from yoink_inbox.services.gh_lists import GhListsClient
    token = await _get_public_repo_token(session_factory, user_id)
    client = GhListsClient(token=token)
    try:
        await client.add_repo_to_list(repo_node_id, list_id)
    finally:
        await client.aclose()
    logger.info(
        "inbox.gh_write: added repo node=%s to list=%s for user_id=%s",
        repo_node_id, list_id, user_id,
    )


async def remove_from_gh_list(
    session_factory,
    user_id: int,
    repo_node_id: str,
    list_id: str,
) -> None:
    """Remove a repo from a GitHub List (safe remove, preserves other memberships)."""
    from yoink_inbox.services.gh_lists import GhListsClient
    token = await _get_public_repo_token(session_factory, user_id)
    client = GhListsClient(token=token)
    try:
        await client.remove_repo_from_list(repo_node_id, list_id)
    finally:
        await client.aclose()
    logger.info(
        "inbox.gh_write: removed repo node=%s from list=%s for user_id=%s",
        repo_node_id, list_id, user_id,
    )
