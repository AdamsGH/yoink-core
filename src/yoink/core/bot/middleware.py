"""Bot middleware helpers: access shared services from context.bot_data.

Prefer the declarative `require_access` decorator from `yoink.core.bot.access`
for new handlers.  The imperative helpers here remain for backward compatibility.
"""
from __future__ import annotations

import logging

from telegram import Update
from telegram.ext import ContextTypes
from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.config import CoreSettings
from yoink.core.db.repos.users import UserRepo
from yoink.core.db.repos.groups import GroupRepo
from yoink.core.db.repos.bot_settings import BotSettingsRepo

logger = logging.getLogger(__name__)


def get_session_factory(context: ContextTypes.DEFAULT_TYPE) -> async_sessionmaker:
    return context.bot_data["session_factory"]


def get_config(context: ContextTypes.DEFAULT_TYPE) -> CoreSettings:
    return context.bot_data["config"]


def get_user_repo(context: ContextTypes.DEFAULT_TYPE) -> UserRepo:
    return context.bot_data["user_repo"]


def get_group_repo(context: ContextTypes.DEFAULT_TYPE) -> GroupRepo | None:
    return context.bot_data.get("group_repo")


def get_bot_settings_repo(context: ContextTypes.DEFAULT_TYPE) -> BotSettingsRepo | None:
    return context.bot_data.get("bot_settings_repo")


async def is_blocked(user_id: int, context: ContextTypes.DEFAULT_TYPE) -> bool:
    """Check if a user is blocked/banned. Uses core UserRepo."""
    repo = get_user_repo(context)
    if repo is None:
        return False
    user = await repo.get_or_create(user_id)
    return user.is_blocked


async def reply_ephemeral(
    update: Update,
    context: ContextTypes.DEFAULT_TYPE,
    text: str,
    delay: float = 8.0,
) -> None:
    """Send a reply visible only briefly in groups (auto-deleted after `delay` seconds).

    In private chats the message is sent normally without deletion.
    Both the command trigger and the bot reply are deleted in groups to keep chat clean.
    """
    msg = update.effective_message
    if not msg:
        return

    chat = update.effective_chat
    is_group = chat and chat.type in ("group", "supergroup")

    sent = await msg.reply_text(text)

    if is_group:
        async def _delete(_ctx: ContextTypes.DEFAULT_TYPE) -> None:
            try:
                await sent.delete()
            except Exception:
                pass
            try:
                await msg.delete()
            except Exception:
                pass

        context.job_queue.run_once(_delete, delay)


async def guard_admin(update: Update, context: ContextTypes.DEFAULT_TYPE) -> bool:
    config = get_config(context)
    user = update.effective_user
    if user and user.id == config.owner_id:
        return True
    repo = get_user_repo(context)
    if repo and user:
        from yoink.core.db.models import UserRole
        u = await repo.get_or_create(
            user.id, username=user.username, first_name=user.first_name,
        )
        if u.role in (UserRole.admin, UserRole.owner):
            return True
    await reply_ephemeral(update, context, "⛔ Not authorized.")
    return False
