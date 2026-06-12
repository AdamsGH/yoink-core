"""`/inbox` shows the user's recent inbox items.

Inline keyboard with one row per item; tap routes to a future detail view.
Phase-1 detail callback is a stub that just toasts the URL.
"""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from sqlalchemy import desc, select
from telegram import InlineKeyboardButton, InlineKeyboardMarkup, Update
from telegram.constants import ParseMode
from telegram.ext import CommandHandler, ContextTypes

from yoink.core.bot.access import AccessPolicy, require_access
from yoink.core.bot.middleware import get_user_repo
from yoink.core.db.models import UserRole
from yoink.core.i18n import t
from yoink_inbox.bot.middleware import get_inbox_config, get_session_factory
from yoink_inbox.storage.models import InboxItem

if TYPE_CHECKING:
    from telegram.ext import Application

logger = logging.getLogger(__name__)

_INBOX_POLICY = AccessPolicy(
    min_role=UserRole.user,
    plugin="inbox",
    feature="ingest",
    silent_deny=False,
    group_silent_deny=True,
)

_STATUS_ICON = {
    "pending":    "○",
    "enriched":   "◐",
    "classified": "●",
    "archived":   "▣",
    "failed":     "✗",
}


def _format_item(item: InboxItem) -> str:
    icon = _STATUS_ICON.get(item.status, "○")
    title = item.title or item.normalized_url
    if len(title) > 56:
        title = title[:55] + "…"
    return f"{icon} {title}"


@require_access(_INBOX_POLICY)
async def _cmd_inbox(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    msg = update.message
    user = update.effective_user
    if msg is None or user is None:
        return

    user_repo = get_user_repo(context)
    db_user = await user_repo.get_or_create(user.id)
    lang = db_user.language

    cfg = get_inbox_config(context)
    limit = cfg.inbox_recent_items_limit

    session_factory = get_session_factory(context)
    async with session_factory() as session:
        items = (
            await session.scalars(
                select(InboxItem)
                .where(InboxItem.user_id == user.id, InboxItem.archived_at.is_(None))
                .order_by(desc(InboxItem.created_at))
                .limit(limit)
            )
        ).all()

    if not items:
        await msg.reply_text(t("inbox.messages.inbox_empty", lang))
        return

    keyboard = [
        [
            InlineKeyboardButton(
                _format_item(item),
                callback_data=f"inbox:item:{item.id}",
            )
        ]
        for item in items
    ]
    await msg.reply_text(
        t("inbox.messages.inbox_header", lang, count=len(items)),
        reply_markup=InlineKeyboardMarkup(keyboard),
        parse_mode=ParseMode.HTML,
    )


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("inbox", _cmd_inbox))
