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
    lang = db_user.language

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

    if arq is None:
        # Falls back to inline enrich so users do not see ghosts when Redis
        # is down. Logged loudly because this is a degraded path.
        logger.warning(
            "inbox.save no ARQ pool, running enrich inline item_id=%s",
            result.item_id,
        )
        from yoink_inbox.services.enrich import run_enrich
        try:
            await run_enrich(session_factory, result.item_id, arq=None)
        except Exception:
            logger.exception("inbox.save inline enrich failed item_id=%s", result.item_id)

    await msg.reply_text(t("inbox.messages.save_ok", lang))


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("save", _cmd_save))
