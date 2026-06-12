"""`/save <url>` and reply-to-save handler.

Inserts the URL into inbox_items (status=pending) and enqueues the enrich
ARQ job. Auto-detect of URLs in any non-command message is deliberately NOT
wired here because yoink-dl already owns that filter in private chats; we
can layer an opt-in user setting on top later.
"""
from __future__ import annotations

import logging
from typing import TYPE_CHECKING

from telegram import Update
from telegram.constants import ParseMode
from telegram.ext import CommandHandler, ContextTypes

from yoink.core.bot.access import AccessPolicy, require_access
from yoink.core.bot.middleware import get_user_repo
from yoink.core.db.models import UserRole
from yoink.core.i18n import t
from yoink_inbox.bot.middleware import get_inbox_arq, get_session_factory
from yoink_inbox.commands._helpers import extract_url_from_args_or_reply
from yoink_inbox.services.ingest import ingest_url

if TYPE_CHECKING:
    from telegram.ext import Application

logger = logging.getLogger(__name__)

_SAVE_POLICY = AccessPolicy(
    min_role=UserRole.user,
    plugin="inbox",
    feature="ingest",
    silent_deny=False,
    group_silent_deny=True,
)


@require_access(_SAVE_POLICY)
async def _cmd_save(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    msg = update.message
    user = update.effective_user
    if msg is None or user is None:
        return

    user_repo = get_user_repo(context)
    db_user = await user_repo.get_or_create(user.id)

    # Prefer insight_access.lang (AI/response language) over system language
    lang = db_user.language
    try:
        session_factory = get_session_factory(context)
        async with session_factory() as _s:
            from yoink_insight.storage.models import InsightAccess  # noqa: PLC0415
            access = await _s.get(InsightAccess, user.id)
            if access and access.lang:
                lang = access.lang
    except Exception:  # noqa: BLE001
        pass

    url = extract_url_from_args_or_reply(context.args or [], msg)
    if url is None:
        await msg.reply_text(
            t("inbox.messages.save_usage", lang),
            parse_mode=ParseMode.HTML,
        )
        return

    session_factory = get_session_factory(context)
    arq = get_inbox_arq(context)

    try:
        async with session_factory() as session:
            result = await ingest_url(
                session,
                user_id=user.id,
                url=url,
                source="bot",
                arq=arq,
            )
            await session.commit()
    except Exception:
        logger.exception("inbox.save failed user_id=%s url=%s", user.id, url)
        await msg.reply_text(t("inbox.messages.save_fail", lang))
        return

    if not result.created:
        await msg.reply_text(t("inbox.messages.save_dup", lang))
        return

    # Send placeholder; after classify the worker will edit it with a summary.
    placeholder = await msg.reply_text(t("inbox.messages.save_ok", lang))

    # Persist tg context so the worker can edit this message later.
    try:
        async with session_factory() as session:
            from yoink_inbox.storage.models import InboxItem  # noqa: PLC0415
            item = await session.get(InboxItem, result.item_id)
            if item is not None:
                item.tg_chat_id = placeholder.chat_id
                item.tg_reply_message_id = placeholder.message_id
                await session.commit()
    except Exception:
        logger.warning("inbox.save could not store tg_reply_message_id item_id=%s", result.item_id)

    if arq is None:
        logger.warning(
            "inbox.save no ARQ pool, running enrich inline item_id=%s",
            result.item_id,
        )
        from yoink_inbox.services.enrich import run_enrich  # noqa: PLC0415
        try:
            await run_enrich(session_factory, result.item_id, arq=None)
        except Exception:
            logger.exception("inbox.save inline enrich failed item_id=%s", result.item_id)


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("save", _cmd_save))
