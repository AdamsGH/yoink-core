"""Plugin activity provider registry.

Each plugin that tracks per-user activity calls register_activity_provider()
in its setup(). Core's /users/:id/stats endpoint queries all registered
providers instead of importing plugin models directly.

Protocol
--------
A provider is an async callable with signature:
    async (session: AsyncSession, user_id: int) -> PluginActivity

PluginActivity is a plain dict with at least:
    plugin  - str, plugin name (e.g. "dl", "music", "insight")
    total   - int, total event count for the user
    last_at - datetime | None, timestamp of most recent event
    extra   - dict[str, Any], plugin-specific fields merged into the response

List-user providers
-------------------
Plugins that add per-user aggregate columns to the user list (e.g. dl_count)
call register_list_users_provider() in setup(). The provider signature is:
    async (session, user_ids: list[int]) -> dict[int, dict]

The returned dict maps user_id -> {field: value, ...}. Core merges these
fields into each UserResponse. Unknown fields are silently ignored by Pydantic.
"""
from __future__ import annotations

import logging
from datetime import datetime
from typing import Any, Awaitable, Callable, TypedDict

from sqlalchemy.ext.asyncio import AsyncSession

logger = logging.getLogger(__name__)


class PluginActivity(TypedDict, total=False):
    plugin: str
    total: int
    last_at: datetime | None
    extra: dict[str, Any]


ActivityProvider = Callable[[AsyncSession, int], Awaitable[PluginActivity]]
ListUsersProvider = Callable[[AsyncSession, list[int]], Awaitable[dict[int, dict]]]

_providers: dict[str, ActivityProvider] = {}
_list_providers: dict[str, ListUsersProvider] = {}


def register_activity_provider(plugin: str, provider: ActivityProvider) -> None:
    """Register an activity provider for a plugin. Idempotent."""
    _providers[plugin] = provider
    logger.debug("Activity provider registered: %s", plugin)


async def collect_activity(session: AsyncSession, user_id: int) -> list[PluginActivity]:
    """Query all registered providers and return their results."""
    results: list[PluginActivity] = []
    for plugin, provider in _providers.items():
        try:
            result = await provider(session, user_id)
            results.append(result)
        except Exception:
            logger.exception("Activity provider %s failed for user %d", plugin, user_id)
    return results


def register_list_users_provider(plugin: str, provider: ListUsersProvider) -> None:
    """Register a list-users provider for a plugin. Idempotent."""
    _list_providers[plugin] = provider
    logger.debug("List-users provider registered: %s", plugin)


async def collect_list_users(session: AsyncSession, user_ids: list[int]) -> dict[int, dict]:
    """Query all list-users providers and merge their results by user_id."""
    merged: dict[int, dict] = {}
    for plugin, provider in _list_providers.items():
        try:
            result = await provider(session, user_ids)
            for uid, fields in result.items():
                merged.setdefault(uid, {}).update(fields)
        except Exception:
            logger.exception("List-users provider %s failed", plugin)
    return merged
