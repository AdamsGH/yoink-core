"""Confusion guard: nudge group users who can't figure out how to use the bot.

Triggers a "just use /help" reply when, within a sliding window of messages
after a bot mention, a user either:
  - asks a "how do I..." style question, or
  - fumbles a command attempt more than twice.

State lives in PTB's chat_data (in-memory, not persisted) under the key
"_confusion_guard" to avoid polluting the DB.
"""
from __future__ import annotations

import logging
import re
import time
from typing import TYPE_CHECKING

from telegram.ext import Application, ContextTypes, MessageHandler, filters

from yoink.core.bot.middleware import get_user_repo
from yoink.core.i18n.loader import t

if TYPE_CHECKING:
    from telegram import Message, Update

logger = logging.getLogger(__name__)

# How many messages from a user (after bot mention) we watch.
WINDOW_SIZE = 5
# How many fumbled-command attempts before we fire.
FUMBLE_THRESHOLD = 2

# Patterns that signal "I'm confused about how to use this".
_CONFUSION_RE = re.compile(
    r"\b(how\s+(do|can|to|should)\b"
    r"|как\s+(мне|это|вызвать|использовать|написать|пользоваться|юзать|работает|работать|запустить|открыть)?"
    r"|как\b"
    r"|что\s+(значит|делает|нужно)"
    r"|не\s+(могу|понимаю|знаю|получается)"
    r"|где\s+(команд|найти|список)"
    r"|can'?t\s+figure"
    r"|don'?t\s+know\s+how"
    r"|what\s+(do|does|is)\b"
    r")",
    re.IGNORECASE,
)

# Patterns that look like a botched command attempt (no slash, or mangled syntax).
_FUMBLE_RE = re.compile(
    r"^[/\\]?\s*[a-zа-яё]{2,20}\s*$"  # bare word that looks like a command name
    r"|^[/\\][a-zа-яё]{2,20}\s+\?\s*$",  # /cmd ? - asking how to use a command
    re.IGNORECASE,
)


def _state(context: ContextTypes.DEFAULT_TYPE, user_id: int) -> dict:
    """Return (and lazily init) per-user tracking state from chat_data."""
    guard = context.chat_data.setdefault("_confusion_guard", {})
    return guard.setdefault(user_id, {"mentions": 0, "window": [], "fired": False})


def _reset(context: ContextTypes.DEFAULT_TYPE, user_id: int) -> None:
    context.chat_data.get("_confusion_guard", {}).pop(user_id, None)


def _is_bot_mention(message: Message, bot_username: str) -> bool:
    """True if the message mentions or addresses the bot.

    Covers: @username mention, reply to bot, or any command sent to the bot
    (either /cmd@username or a bare /cmd in a group where bot is present).
    """
    if message.text:
        text = message.text.lower()
        if f"@{bot_username}".lower() in text:
            return True
        # /cmd@botname - explicitly addressed command
        if re.match(rf"^/[a-z_]+@{re.escape(bot_username.lower())}", text):
            return True
    return bool(
        message.reply_to_message
        and message.reply_to_message.from_user
        and message.reply_to_message.from_user.username == bot_username
    )


def _is_any_command(message: Message) -> bool:
    """True if the message is a bot command."""
    return bool(message.text and message.text.startswith("/"))


def _is_fumble(text: str) -> bool:
    """True if the message looks like a mangled command attempt."""
    return bool(_FUMBLE_RE.match(text.strip()))


def _is_confused(text: str) -> bool:
    return bool(_CONFUSION_RE.search(text))


async def _maybe_fire(
    msg: Message,
    user_id: int,
    context: ContextTypes.DEFAULT_TYPE,
    st: dict,
) -> None:
    """Check window state and send nudge if threshold is reached."""
    fumble_count = sum(1 for e in st["window"] if e["fumble"])
    has_confusion_signal = any(e["confused"] for e in st["window"])

    if not (has_confusion_signal or fumble_count > FUMBLE_THRESHOLD):
        return

    st["fired"] = True

    lang = "en"
    repo = get_user_repo(context)
    if repo:
        try:
            db_user = await repo.get_or_create(user_id)
            lang = db_user.language
        except Exception:
            pass

    await msg.reply_text(t("confusion_guard.nudge", lang))


async def _handle_group_command(
    update: Update, context: ContextTypes.DEFAULT_TYPE
) -> None:
    """Handle commands in groups: open a tracking window and count as a fumble."""
    msg = update.message
    if not msg or not update.effective_user:
        return

    user = update.effective_user
    st = _state(context, user.id)

    if st["fired"]:
        # Reset on a new command so the user gets a second chance after the nudge.
        _reset(context, user.id)
        st = _state(context, user.id)

    # Every command to the bot opens (or resets) the window.
    st["mentions"] += 1
    st["window"] = []
    st["fired"] = False
    # The command itself counts as one fumble attempt.
    st["window"].append({"confused": False, "fumble": True, "ts": time.time()})


async def _handle_group_message(
    update: Update, context: ContextTypes.DEFAULT_TYPE
) -> None:
    msg = update.message
    if not msg or not update.effective_user:
        return

    user = update.effective_user
    text = (msg.text or msg.caption or "").strip()
    if not text:
        return

    bot_username: str = context.bot.username or ""
    st = _state(context, user.id)

    # If the user already got the nudge recently, cool down until next command/mention.
    if st["fired"]:
        if _is_bot_mention(msg, bot_username):
            _reset(context, user.id)
        return

    # Open (or refresh) a window on @mention or reply to bot.
    if _is_bot_mention(msg, bot_username):
        st["mentions"] += 1
        st["window"] = []
        st["fired"] = False
        return

    # Only track users who have interacted with the bot.
    if st["mentions"] == 0:
        return

    if len(st["window"]) >= WINDOW_SIZE:
        _reset(context, user.id)
        return

    confused = _is_confused(text)
    fumble = _is_fumble(text)

    st["window"].append({"confused": confused, "fumble": fumble, "ts": time.time()})

    await _maybe_fire(msg, user.id, context, st)


def register(app: Application) -> None:
    app.add_handler(
        MessageHandler(
            filters.ChatType.GROUPS & filters.COMMAND,
            _handle_group_command,
        ),
        group=99,
    )
    app.add_handler(
        MessageHandler(
            filters.ChatType.GROUPS & filters.TEXT & ~filters.COMMAND,
            _handle_group_message,
        ),
        group=99,
    )
