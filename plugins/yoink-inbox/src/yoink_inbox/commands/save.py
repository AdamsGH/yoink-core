"""`/save <url>` handler - inline enrich+classify pipeline, reply when done.

Mirrors the tldr pattern: the command handler runs the full pipeline inline
(ingest -> enrich -> classify) while holding a `typing` chat action, then
replies once with the final summary + category tags.  No ARQ job is enqueued
for the primary path; ARQ is kept as a fallback only when Redis is present
and the inline path fails (for fire-and-forget retries).
"""
from __future__ import annotations

import asyncio
import logging
from typing import TYPE_CHECKING

from telegram import Update
from telegram.constants import ChatAction, ParseMode
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

# How often to re-send the typing action while the pipeline runs (seconds).
_TYPING_INTERVAL = 4.0


async def _keep_typing(bot, chat_id: int, stop_event: asyncio.Event) -> None:
    """Periodically send typing action until stop_event is set."""
    try:
        while not stop_event.is_set():
            await bot.send_chat_action(chat_id=chat_id, action=ChatAction.TYPING)
            try:
                await asyncio.wait_for(
                    asyncio.shield(stop_event.wait()),
                    timeout=_TYPING_INTERVAL,
                )
            except asyncio.TimeoutError:
                pass
    except Exception:  # noqa: BLE001
        pass


async def _resolve_lang(user_id: int, fallback: str, session_factory) -> str:
    """Return the user's AI response language from insight_access, or fallback."""
    try:
        async with session_factory() as session:
            from yoink_insight.storage.models import InsightAccess  # noqa: PLC0415
            row = await session.get(InsightAccess, user_id)
            if row and row.lang:
                return row.lang
    except Exception:  # noqa: BLE001
        pass
    return fallback


def _build_reply(
    title: str | None,
    url: str,
    summary: str | None,
    categories: list[str],
) -> str:
    """Format the final reply text: summary + hashtag list."""
    cat_tags = " ".join(f"#{c.replace(' ', '_')}" for c in categories) if categories else ""
    lines: list[str] = []
    if summary:
        lines.append(summary)
    if cat_tags:
        lines.append(cat_tags)
    return "\n\n".join(lines) if lines else (title or url)


@require_access(_SAVE_POLICY)
async def _cmd_save(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    msg = update.message
    user = update.effective_user
    if msg is None or user is None:
        return

    user_repo = get_user_repo(context)
    db_user = await user_repo.get_or_create(user.id)
    session_factory = get_session_factory(context)
    arq = get_inbox_arq(context)

    lang = await _resolve_lang(user.id, db_user.language, session_factory)

    url = extract_url_from_args_or_reply(context.args or [], msg)
    if url is None:
        await msg.reply_text(
            t("inbox.messages.save_usage", lang),
            parse_mode=ParseMode.HTML,
        )
        return

    # --- ingest (dedup check) ---
    try:
        async with session_factory() as session:
            result = await ingest_url(
                session,
                user_id=user.id,
                url=url,
                source="bot",
                arq=None,  # we run inline; ARQ not needed for enqueue here
            )
            await session.commit()
    except Exception:
        logger.exception("inbox.save failed user_id=%s url=%s", user.id, url)
        await msg.reply_text(t("inbox.messages.save_fail", lang))
        return

    if not result.created:
        await msg.reply_text(t("inbox.messages.save_dup", lang))
        return

    item_id = result.item_id

    # --- inline enrich + classify with typing indicator ---
    stop_typing = asyncio.Event()
    typing_task = asyncio.create_task(
        _keep_typing(context.bot, msg.chat_id, stop_typing)
    )

    summary: str | None = None
    categories: list[str] = []
    title: str | None = None

    try:
        from yoink_inbox.services.enrich import run_enrich  # noqa: PLC0415
        from yoink_inbox.services.classify import run_classify  # noqa: PLC0415
        from yoink_inbox.storage.models import InboxItem  # noqa: PLC0415
        from yoink_inbox.storage.models import InboxItemCategory  # noqa: PLC0415
        from yoink_inbox.storage.models import InboxCategory  # noqa: PLC0415
        from sqlalchemy import select  # noqa: PLC0415

        await run_enrich(session_factory, item_id, arq=None)
        await run_classify(session_factory, item_id, notify=False)

        async with session_factory() as session:
            item = await session.get(InboxItem, item_id)
            if item:
                summary = item.summary
                title = item.title
            cat_rows = (await session.execute(
                select(InboxCategory.name)
                .join(InboxItemCategory, InboxItemCategory.category_id == InboxCategory.id)
                .where(InboxItemCategory.item_id == item_id)
                .order_by(InboxItemCategory.confidence.desc().nullslast())
            )).scalars().all()
            categories = list(cat_rows)
    except Exception:
        logger.exception("inbox.save inline pipeline failed item_id=%s", item_id)
        # Enqueue for retry via ARQ if available
        if arq is not None:
            try:
                await arq.enqueue_job("enrich_item", item_id, _queue_name="inbox:default")
            except Exception:
                logger.warning("inbox.save ARQ fallback enqueue failed item_id=%s", item_id)
        await msg.reply_text(t("inbox.messages.save_fail", lang))
        return
    finally:
        stop_typing.set()
        await typing_task

    reply_text = _build_reply(title, url, summary, categories)
    await msg.reply_text(reply_text)


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("save", _cmd_save))
