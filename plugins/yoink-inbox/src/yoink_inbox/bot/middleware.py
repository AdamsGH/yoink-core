"""Accessors over PTB `context.bot_data` for inbox services.

Every key here is populated in `InboxPlugin.setup(ctx)`. Centralising the
getters makes refactor-friendly typed access and a single place to grep for
all bot_data keys belonging to the plugin.
"""
from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from arq.connections import ArqRedis
    from sqlalchemy.ext.asyncio import async_sessionmaker
    from telegram.ext import ContextTypes

    from yoink_inbox.config import InboxConfig


_BOT_DATA_CONFIG = "inbox_config"
_BOT_DATA_ARQ = "inbox_arq_pool"
_BOT_DATA_SESSION_FACTORY = "session_factory"


def get_inbox_config(context: "ContextTypes.DEFAULT_TYPE") -> "InboxConfig":
    return context.bot_data[_BOT_DATA_CONFIG]


def get_inbox_arq(context: "ContextTypes.DEFAULT_TYPE") -> "ArqRedis | None":
    """Return the inbox ARQ pool. None if startup did not wire it (Redis down)."""
    return context.bot_data.get(_BOT_DATA_ARQ)


def get_session_factory(context: "ContextTypes.DEFAULT_TYPE") -> "async_sessionmaker":
    """Pulled from core's bot_data (set up by yoink core, not the plugin)."""
    return context.bot_data[_BOT_DATA_SESSION_FACTORY]
