"""/stars_sync command: enqueue a one-off GitHub starred-repos sync.

The actual GH API hop happens in the ARQ worker (`sync_user_stars`); this
handler only enqueues. The bot replies immediately so the user is not
blocked while pagination runs. Status updates land via /inbox or the
upcoming web UI.

If Redis is down we fall back to inline sync the same way /save does for
enrich; verbose log, user still gets a response, just slower.
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

if TYPE_CHECKING:
    from telegram.ext import Application

logger = logging.getLogger(__name__)

_STARS_SYNC_POLICY = AccessPolicy(
    min_role=UserRole.user,
    plugin="inbox",
    feature="gh_sync",
    silent_deny=False,
    group_silent_deny=True,
)


@require_access(_STARS_SYNC_POLICY)
async def _cmd_stars_sync(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    msg = update.message
    user = update.effective_user
    if msg is None or user is None:
        return

    user_repo = get_user_repo(context)
    db_user = await user_repo.get_or_create(user.id)
    lang = db_user.language

    arq = get_inbox_arq(context)
    session_factory = get_session_factory(context)

    if arq is not None:
        await arq.enqueue_job(
            "sync_user_stars",
            user.id,
            _queue_name="inbox:default",
        )
        await msg.reply_text(
            t("inbox.messages.stars_sync_started", lang),
            parse_mode=ParseMode.HTML,
        )
        return

    # Redis down: run inline. Slow but at least it works.
    logger.warning("inbox.stars_sync no ARQ pool, running inline user_id=%s", user.id)
    from yoink_inbox.services.gh_stars import run_sync

    try:
        result = await run_sync(session_factory, user.id)
    except Exception:
        logger.exception("inbox.stars_sync inline failed user_id=%s", user.id)
        await msg.reply_text(t("inbox.messages.stars_sync_fail", lang))
        return

    await msg.reply_text(
        t(
            "inbox.messages.stars_sync_done",
            lang,
            status=result.status,
            count=result.stars_count,
            removed=result.removed,
        ),
        parse_mode=ParseMode.HTML,
    )


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("stars_sync", _cmd_stars_sync))
