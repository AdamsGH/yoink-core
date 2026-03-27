"""Core bot commands: /start, /help, /lang."""
from __future__ import annotations

from telegram import (
    CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup,
    LinkPreviewOptions, Update,
)
from telegram.ext import (
    Application, CallbackQueryHandler, CommandHandler, ContextTypes, filters,
)

from yoink.core.bot.bot_commands import refresh_user_commands
from yoink.core.bot.middleware import get_user_repo
from yoink.core.db.models import UserRole
from yoink.core.i18n.loader import SUPPORTED, t


async def _cmd_start(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    repo = get_user_repo(context)
    tg = update.effective_user
    user = await repo.get_or_create(
        tg.id,
        username=tg.username,
        first_name=tg.first_name,
        is_premium=bool(getattr(tg, "is_premium", False)),
    )
    if user.role == UserRole.restricted:
        await update.message.reply_html(t("start.pending", user.language))
        return
    if user.is_blocked:
        await update.message.reply_html(t("start.banned", user.language))
        return

    session_factory = context.bot_data.get("session_factory")

    class _AppState:
        bot = context.bot
        bot_data = context.bot_data

    await refresh_user_commands(
        _AppState(),
        update.effective_user.id,
        role=user.role.value,
        lang=user.language,
        session_factory=session_factory,
    )

    await update.message.reply_html(
        t("start.welcome", user.language),
        link_preview_options=LinkPreviewOptions(is_disabled=True),
    )


async def _cmd_help(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    tg = update.effective_user
    repo = get_user_repo(context)
    user = await repo.get_or_create(tg.id, username=tg.username, first_name=tg.first_name)

    plugins = context.bot_data.get("plugins", [])
    lines = [t("help.title", user.language)]
    for plugin in plugins:
        if hasattr(plugin, "get_help_section"):
            section = plugin.get_help_section(user.role.value, user.language)
            if section:
                lines.append(section)
    await update.message.reply_html("\n\n".join(lines))


def _lang_keyboard(current: str) -> InlineKeyboardMarkup:
    buttons = [
        InlineKeyboardButton(
            f"{t(f'lang.buttons.{code}', current)}{'  ✓' if code == current else ''}",
            callback_data=f"lang:{code}",
        )
        for code in sorted(SUPPORTED)
    ]
    rows = [buttons[i : i + 2] for i in range(0, len(buttons), 2)]
    return InlineKeyboardMarkup(rows)


async def _cmd_lang(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_user:
        return
    tg = update.effective_user
    repo = get_user_repo(context)
    user = await repo.get_or_create(tg.id, username=tg.username, first_name=tg.first_name)
    await update.message.reply_html(
        t("lang.choose", user.language),
        reply_markup=_lang_keyboard(user.language),
    )


async def _cb_lang(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    query: CallbackQuery | None = update.callback_query
    if not query or not update.effective_user:
        return
    await query.answer()
    lang_code = (query.data or "").split(":", 1)[1]
    if lang_code not in SUPPORTED:
        return
    repo = get_user_repo(context)
    await repo.update(update.effective_user.id, language=lang_code)
    await query.edit_message_text(
        t("lang.set", lang_code, lang=t(f"lang.buttons.{lang_code}", lang_code)),
        reply_markup=None,
        parse_mode="HTML",
    )


def register(app: Application) -> None:
    app.add_handler(CommandHandler("start", _cmd_start, filters=filters.ChatType.PRIVATE))
    app.add_handler(CommandHandler("help", _cmd_help, filters=filters.ChatType.PRIVATE))
    app.add_handler(CommandHandler("lang", _cmd_lang))
    app.add_handler(CallbackQueryHandler(_cb_lang, pattern=r"^lang:"))
