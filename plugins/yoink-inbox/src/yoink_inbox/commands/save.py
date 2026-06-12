"""`/save <url>` handler - inline enrich+classify pipeline, streamed reply.

Pipeline: ingest -> enrich -> classify (inline, with typing action).
Reply: streamed via send_message_draft (Bot API 10.0) + final send_message,
using the nobullshit alias prompt and format rules from yoink-insight.
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

# Mirrors tldr.py constants
_DRAFT_MIN_CHARS = 80
_DRAFT_MIN_INTERVAL = 1.5
_TG_DRAFT_LIMIT_U16 = 3800
_TYPING_INTERVAL = 4.0

# Prompt for the post-save comment - reuses nobullshit voice + alias format rules
_SAVE_PROMPT = """\
{nobullshit}

Below is an item just saved to the user's inbox. Give ONE punchy sentence \
(max two) as a direct verdict on the topic itself - what this thing actually \
is or does, not that it was saved.
- No mention of saving, bookmarking, or the inbox
- No hollow openers ("Interesting!", "Worth reading", "A solid...")
- Concrete claim or observation, craftsman voice
- After your sentence, on a new line: category hashtags only

Categories: {categories}
Title: {title}
Summary: {summary}

{format_rules}
Reply in {lang}.
"""


async def _keep_typing(bot, chat_id: int, stop_event: asyncio.Event) -> None:
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
    try:
        async with session_factory() as session:
            from yoink_insight.storage.models import InsightAccess  # noqa: PLC0415
            row = await session.get(InsightAccess, user_id)
            if row and row.lang:
                return row.lang
    except Exception:  # noqa: BLE001
        pass
    return fallback


def _utf16_len(s: str) -> int:
    return len(s.encode("utf-16-le")) // 2


async def _stream_comment(
    bot,
    chat_id: int,
    reply_to_message_id: int,
    draft_id: int,
    title: str | None,
    url: str,
    summary: str | None,
    categories: list[str],
    lang: str,
    session_factory,
    user_id: int,
) -> None:
    """Stream the post-save comment via send_message_draft, then send_message."""
    from yoink_insight.config import InsightConfig  # noqa: PLC0415
    from yoink_insight.services.tldr import (  # noqa: PLC0415
        PreparedTldr, _NOBULLSHIT_PROMPT, _ALIAS_FORMAT_RULES, stream_llm,
    )
    from yoink_insight.services.md_entities import md_to_entities  # noqa: PLC0415

    config = InsightConfig()
    # Tags as monospace code block so Telegram renders them properly
    cat_tags = "`" + " ".join(f"#{c.replace(' ', '_')}" for c in categories) + "`" if categories else ""

    # Build a minimal PreparedTldr from the already-enriched item content
    content_body = f"Title: {title or url}\nSummary: {summary or '(none)'}"
    prepared = PreparedTldr(
        content=content_body,
        source_desc=title or url,
        is_youtube=False,
        video_seconds=None,
        via="inbox",
    )

    # Inject nobullshit + format rules into a custom instruction override
    nobullshit = _NOBULLSHIT_PROMPT.replace("{lang}", lang)
    prompt_instruction = _SAVE_PROMPT.format(
        nobullshit=nobullshit,
        categories=", ".join(categories) if categories else "none",
        title=title or url,
        summary=summary or "(none)",
        format_rules=_ALIAS_FORMAT_RULES,
        lang=lang,
    )

    accumulated = ""
    last_sent_len = 0
    last_sent_at = 0.0
    draft_disabled = False

    try:
        async for chunk in stream_llm(
            prepared, lang, config,
            default_instruction_override=prompt_instruction,
        ):
            accumulated += chunk
            now = asyncio.get_event_loop().time()
            if draft_disabled:
                continue
            if (
                len(accumulated) - last_sent_len >= _DRAFT_MIN_CHARS
                and now - last_sent_at >= _DRAFT_MIN_INTERVAL
            ):
                try:
                    if _utf16_len(accumulated) > _TG_DRAFT_LIMIT_U16:
                        draft_disabled = True
                        continue
                    if md_to_entities is not None:
                        draft_plain, _ = md_to_entities(accumulated.strip())
                    else:
                        draft_plain = accumulated.strip()
                    await bot.send_message_draft(
                        chat_id=chat_id,
                        draft_id=draft_id,
                        text=draft_plain,
                    )
                    last_sent_len = len(accumulated)
                    last_sent_at = now
                except Exception as e:  # noqa: BLE001
                    logger.debug("send_message_draft failed: %s", e)
    except Exception:
        logger.exception("inbox.save stream_llm failed user_id=%s", user_id)
        # Fall back to plain summary+tags
        accumulated = f"{summary}\n\n{cat_tags}" if summary else cat_tags

    # Strip any hashtag lines the LLM may have added - we append ours canonically
    body_lines = accumulated.strip().splitlines()
    clean_lines = [ln for ln in body_lines if not ln.strip().startswith("#")]
    body = "\n".join(clean_lines).strip()
    if not body:
        body = title or url

    # Tags as monospace last line
    if cat_tags:
        body = f"{body}\n\n{cat_tags}"

    # Final send_message via md_to_entities (handles em-dash strip, entity offsets)
    plain, entities = md_to_entities(body)
    try:
        await bot.send_message(
            chat_id=chat_id,
            text=plain,
            entities=entities or None,
            reply_to_message_id=reply_to_message_id,
        )
    except Exception:
        logger.warning("inbox.save send_message entities failed, retrying plain")
        await bot.send_message(
            chat_id=chat_id,
            text=plain,
            reply_to_message_id=reply_to_message_id,
        )


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

    # --- ingest ---
    try:
        async with session_factory() as session:
            result = await ingest_url(
                session,
                user_id=user.id,
                url=url,
                source="bot",
                arq=None,
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

    # --- streamed comment in nobullshit voice ---
    await _stream_comment(
        bot=context.bot,
        chat_id=msg.chat_id,
        reply_to_message_id=msg.message_id,
        draft_id=msg.message_id,
        title=title,
        url=url,
        summary=summary,
        categories=categories,
        lang=lang,
        session_factory=session_factory,
        user_id=user.id,
    )


def register(app: "Application") -> None:
    app.add_handler(CommandHandler("save", _cmd_save))
