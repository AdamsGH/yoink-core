"""Core admin commands: /block, /unblock, /ban_time, /broadcast, /runtime.

These commands operate on core models only (User, UserRepo) and are available
regardless of which plugins are loaded.

Usage:
  /block <user_id> [reason]
  /unblock <user_id>
  /ban_time <user_id> <duration>   e.g. 1h 30m 7d
  /broadcast <text>
  /runtime
"""
from __future__ import annotations

import asyncio
import logging
import time

from sqlalchemy import select

from telegram import Update
from telegram.ext import Application, CommandHandler, ContextTypes

from yoink.core.bot.access import AccessPolicy, require_access
from yoink.core.bot.middleware import get_session_factory, get_user_repo
from yoink.core.db.models import User, UserRole
from yoink.core.i18n import t
from yoink.core.utils.formatting import humantime, parse_duration

logger = logging.getLogger(__name__)

_start_time = time.monotonic()

_ADMIN_POLICY = AccessPolicy(min_role=UserRole.admin, silent_deny=True)


@require_access(_ADMIN_POLICY)
async def _cmd_block(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    args = context.args or []
    if not args or not args[0].isdigit():
        await update.message.reply_html(
            "Usage: <code>/block &lt;user_id&gt; [reason]</code>"
        )
        return
    target_id = int(args[0])
    repo = get_user_repo(context)
    await repo.update(target_id, role=UserRole.banned)
    lang = (await repo.get_or_create(update.effective_user.id)).language
    await update.message.reply_html(t("admin.user_blocked", lang, user_id=target_id))
    logger.info("User %d blocked by admin %d", target_id, update.effective_user.id)


@require_access(_ADMIN_POLICY)
async def _cmd_unblock(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    args = context.args or []
    if not args or not args[0].isdigit():
        await update.message.reply_html(
            "Usage: <code>/unblock &lt;user_id&gt;</code>"
        )
        return
    target_id = int(args[0])
    repo = get_user_repo(context)
    await repo.update(target_id, role=UserRole.user, ban_until=None)
    lang = (await repo.get_or_create(update.effective_user.id)).language
    await update.message.reply_html(t("admin.user_unblocked", lang, user_id=target_id))
    logger.info("User %d unblocked by admin %d", target_id, update.effective_user.id)


@require_access(_ADMIN_POLICY)
async def _cmd_ban_time(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    args = context.args or []
    if len(args) < 2 or not args[0].isdigit():
        await update.message.reply_html(
            "Usage: <code>/ban_time &lt;user_id&gt; &lt;duration&gt;</code>\n"
            "Duration: <code>30m</code>, <code>1h</code>, <code>7d</code>, <code>2h30m</code>"
        )
        return
    target_id = int(args[0])
    duration_str = " ".join(args[1:])
    delta = parse_duration(duration_str)
    repo = get_user_repo(context)
    lang = (await repo.get_or_create(update.effective_user.id)).language
    if not delta:
        await update.message.reply_html(t("admin.invalid_duration", lang))
        return
    from datetime import datetime, timezone
    ban_until = datetime.now(timezone.utc) + delta
    await repo.update(target_id, ban_until=ban_until)
    await update.message.reply_html(
        t("admin.ban_set", lang, user_id=target_id, duration=duration_str)
    )


@require_access(_ADMIN_POLICY)
async def _cmd_broadcast(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    args = context.args or []
    text = " ".join(args).strip()
    if not text:
        await update.message.reply_html(
            "Usage: <code>/broadcast &lt;message text&gt;</code>"
        )
        return
    session_factory = get_session_factory(context)
    async with session_factory() as session:
        result = await session.execute(
            select(User.id).where(User.role != UserRole.banned)
        )
        user_ids = [row[0] for row in result.fetchall()]
    repo = get_user_repo(context)
    lang = (await repo.get_or_create(update.effective_user.id)).language
    status = await update.message.reply_html(
        t("admin.broadcast_started", lang, count=len(user_ids))
    )

    async def _do_broadcast() -> None:
        sent = failed = 0
        for uid in user_ids:
            try:
                await context.bot.send_message(uid, text)
                sent += 1
            except Exception:
                failed += 1
            # 20 msg/s per-chat limit; 0.05s gives comfortable headroom
            await asyncio.sleep(0.05)
        await status.edit_text(
            t("admin.broadcast_done", lang, sent=sent, failed=failed),
            parse_mode="HTML",
        )

    context.application.create_task(_do_broadcast())


@require_access(_ADMIN_POLICY)
async def _cmd_runtime(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    elapsed_ms = (time.monotonic() - _start_time) * 1000
    repo = get_user_repo(context)
    lang = (await repo.get_or_create(update.effective_user.id)).language
    await update.message.reply_html(
        t("admin.runtime", lang, uptime=humantime(elapsed_ms))
    )


def register(app: Application) -> None:
    app.add_handler(CommandHandler("block", _cmd_block))
    app.add_handler(CommandHandler("unblock", _cmd_unblock))
    app.add_handler(CommandHandler("ban_time", _cmd_ban_time))
    app.add_handler(CommandHandler("broadcast", _cmd_broadcast))
    app.add_handler(CommandHandler("runtime", _cmd_runtime))
