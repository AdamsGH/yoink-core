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

_providers: dict[str, ActivityProvider] = {}


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
