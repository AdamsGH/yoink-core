"""Bot command scope registration.

Scopes used in CommandSpec:
  "default"  - shown in private chats AND group chats (user-level)
  "private"  - shown only in private chats
  "groups"   - shown only in groups, respects min_role via AllChatAdministrators scope

Language support:
  set_default_commands registers commands for every language that has translations
  in any CommandSpec.descriptions dict, in addition to the default (no language_code).
  Telegram shows the localized list to users whose interface language matches.
"""
from __future__ import annotations

import logging

from telegram import (
    Bot, BotCommand,
    BotCommandScopeAllChatAdministrators,
    BotCommandScopeAllGroupChats,
    BotCommandScopeChat,
    BotCommandScopeDefault,
)
from telegram.error import TelegramError

from yoink.core.plugin import CommandSpec

logger = logging.getLogger(__name__)

_ROLE_RANK: dict[str, int] = {
    "user": 0,
    "moderator": 1,
    "admin": 2,
    "owner": 3,
}

_CORE_COMMANDS: list[CommandSpec] = [
    CommandSpec("start",     "Welcome",                  scope="default"),
    CommandSpec("help",      "Show help",                scope="default"),
    CommandSpec("lang",      "Interface language",       scope="private"),
    CommandSpec("block",     "Block a user",             scope="private", min_role="admin"),
    CommandSpec("unblock",   "Unblock a user",           scope="private", min_role="admin"),
    CommandSpec("ban_time",  "Temporarily ban a user",   scope="private", min_role="admin"),
    CommandSpec("broadcast", "Broadcast a message",      scope="private", min_role="admin"),
    CommandSpec("group",     "Group access control",     scope="groups",  min_role="admin"),
    CommandSpec("thread",    "Thread access control",    scope="groups",  min_role="admin"),
    CommandSpec("runtime",   "Bot runtime info",         scope="private", min_role="owner"),
]


def _filter_by_role(commands: list[CommandSpec], role: str) -> list[CommandSpec]:
    rank = _ROLE_RANK.get(role, 0)
    return [c for c in commands if _ROLE_RANK.get(c.min_role, 0) <= rank]


def _collect_languages(commands: list[CommandSpec]) -> set[str]:
    """Return all language codes present in any CommandSpec.descriptions."""
    langs: set[str] = set()
    for cmd in commands:
        langs.update(cmd.descriptions.keys())
    return langs


def _make_bot_commands(commands: list[CommandSpec], lang: str | None = None) -> list[BotCommand]:
    """Build BotCommand list, using localized description when available."""
    result = []
    for c in commands:
        if lang and lang in c.descriptions:
            desc = c.descriptions[lang]
        else:
            desc = c.description
        result.append(BotCommand(c.command, desc))
    return result


async def _set_commands_for_langs(
    bot: Bot,
    commands: list[BotCommand],
    scope: object,
    all_commands: list[CommandSpec],
    scope_filter_fn,
    role_filter: str,
    scope_label: str,
) -> None:
    """Register a command set for default locale and all known language locales."""
    filtered = [c for c in _filter_by_role(all_commands, role_filter) if scope_filter_fn(c)]
    if not filtered:
        return

    default_cmds = _make_bot_commands(filtered)
    try:
        await bot.set_my_commands(default_cmds, scope=scope)
        logger.info("%s commands set (%d)", scope_label, len(default_cmds))
    except TelegramError as e:
        logger.warning("Failed to set %s commands: %s", scope_label, e)

    for lang in _collect_languages(all_commands):
        lang_cmds = _make_bot_commands(filtered, lang=lang)
        try:
            await bot.set_my_commands(lang_cmds, scope=scope, language_code=lang)
            logger.debug("%s commands set for lang=%s (%d)", scope_label, lang, len(lang_cmds))
        except TelegramError as e:
            logger.warning("Failed to set %s commands for lang=%s: %s", scope_label, lang, e)


async def set_default_commands(
    bot: Bot,
    plugin_commands: list[CommandSpec] | None = None,
) -> None:
    """Register commands for all scopes and all known language locales."""
    all_commands = _CORE_COMMANDS + (plugin_commands or [])

    # Default scope - user-visible commands shown before /start
    await _set_commands_for_langs(
        bot, [], BotCommandScopeDefault(), all_commands,
        lambda c: c.scope == "default", "user", "Default",
    )

    # All group chats - user-level commands that make sense in groups
    await _set_commands_for_langs(
        bot, [], BotCommandScopeAllGroupChats(), all_commands,
        lambda c: c.scope == "default", "user", "AllGroupChats",
    )

    # All group chat administrators - user commands + groups-scoped admin commands
    await _set_commands_for_langs(
        bot, [], BotCommandScopeAllChatAdministrators(), all_commands,
        lambda c: c.scope in ("default", "groups"), "owner", "AllChatAdmins",
    )


async def set_user_commands(
    bot: Bot,
    chat_id: int,
    role: str,
    plugin_commands: list[CommandSpec] | None = None,
    lang: str | None = None,
) -> None:
    """Set per-chat command list for a private chat based on user role and language."""
    all_commands = _CORE_COMMANDS + (plugin_commands or [])
    scope = BotCommandScopeChat(chat_id=chat_id)

    if role == "banned":
        try:
            await bot.delete_my_commands(scope=scope)
        except TelegramError as e:
            logger.warning("Failed to clear commands for chat %d: %s", chat_id, e)
        return

    visible = _filter_by_role(all_commands, role)
    cmds = _make_bot_commands(
        [c for c in visible if c.scope in ("default", "private")],
        lang=lang,
    )
    try:
        await bot.set_my_commands(cmds, scope=scope)
        logger.debug("Commands set for chat %d (role=%s lang=%s): %d", chat_id, role, lang, len(cmds))
    except TelegramError as e:
        logger.warning("Failed to set commands for chat %d: %s", chat_id, e)
