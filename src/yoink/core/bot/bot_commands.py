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
    CommandSpec("start",     "Welcome",                  scope="default",  descriptions={"ru": "Начало работы"}),
    CommandSpec("help",      "Show help",                scope="default",  descriptions={"ru": "Список команд"}),
    CommandSpec("lang",      "Interface language",       scope="private",  descriptions={"ru": "Язык интерфейса"}),
    CommandSpec("block",     "Block a user",             scope="private",  min_role="admin", descriptions={"ru": "Заблокировать пользователя"}),
    CommandSpec("unblock",   "Unblock a user",           scope="private",  min_role="admin", descriptions={"ru": "Разблокировать пользователя"}),
    CommandSpec("ban_time",  "Temporarily ban a user",   scope="private",  min_role="admin", descriptions={"ru": "Временная блокировка"}),
    CommandSpec("broadcast", "Broadcast a message",      scope="private",  min_role="admin", descriptions={"ru": "Рассылка сообщения"}),
    CommandSpec("group",     "Group access control",     scope="groups",   min_role="admin", descriptions={"ru": "Управление группой"}),
    CommandSpec("thread",    "Thread access control",    scope="groups",   min_role="admin", descriptions={"ru": "Управление тредом"}),
    CommandSpec("runtime",   "Bot runtime info",         scope="private",  min_role="owner", descriptions={"ru": "Информация о боте"}),
]


def _filter_by_role(
    commands: list[CommandSpec],
    role: str,
    granted_features: set[str] | None = None,
) -> list[CommandSpec]:
    """Filter commands by role rank and optional feature grants.

    granted_features: set of "plugin:feature" strings the user has access to.
    When a CommandSpec has required_feature set, it is only included if that
    feature is in granted_features (or granted_features is None, meaning skip
    feature filtering entirely for backwards-compat callers).
    """
    rank = _ROLE_RANK.get(role, 0)
    out = []
    for c in commands:
        if _ROLE_RANK.get(c.min_role, 0) > rank:
            continue
        if c.required_feature is not None and granted_features is not None:
            if c.required_feature not in granted_features:
                continue
        out.append(c)
    return out


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
    granted_features: set[str] | None = None,
) -> None:
    """Set per-chat command list for a private chat based on user role and language.

    Registers two scopes:
    - BotCommandScopeChat (no lang) - base fallback for this chat
    - BotCommandScopeChat + language_code - highest priority per Telegram docs

    granted_features: set of "plugin:feature" strings. Commands with
    required_feature are only shown when the user has that feature granted.
    Pass None to skip feature filtering (backwards compat).
    """
    all_commands = _CORE_COMMANDS + (plugin_commands or [])
    scope = BotCommandScopeChat(chat_id=chat_id)

    # Always clear all known per-language scopes to remove stale entries from
    # previous language settings or old bot versions.
    _KNOWN_LANGS = ("en", "ru")
    for stale_lang in _KNOWN_LANGS:
        try:
            await bot.delete_my_commands(scope=scope, language_code=stale_lang)
        except TelegramError:
            pass

    if role == "banned":
        try:
            await bot.delete_my_commands(scope=scope)
        except TelegramError as e:
            logger.warning("Failed to clear commands for chat %d: %s", chat_id, e)
        return

    visible = _filter_by_role(all_commands, role, granted_features=granted_features)
    private_cmds = [c for c in visible if c.scope in ("default", "private")]

    # Base scope (no lang) - shown to clients whose Telegram language has no
    # explicit scope set. Use the user's preferred language so that regardless
    # of Telegram client locale the user sees their chosen language.
    base_cmds = _make_bot_commands(private_cmds, lang=lang)
    try:
        await bot.set_my_commands(base_cmds, scope=scope)
        logger.debug("Commands set for chat %d (role=%s no-lang→%s): %d", chat_id, role, lang or "en", len(base_cmds))
    except TelegramError as e:
        logger.warning("Failed to set commands for chat %d: %s", chat_id, e)

    # Language-specific scope - explicit match for clients whose Telegram locale
    # matches the user's preferred language (highest priority in lookup chain).
    if lang:
        lang_cmds = _make_bot_commands(private_cmds, lang=lang)
        try:
            await bot.set_my_commands(lang_cmds, scope=scope, language_code=lang)
            logger.debug("Commands set for chat %d (role=%s lang=%s): %d", chat_id, role, lang, len(lang_cmds))
        except TelegramError as e:
            logger.warning("Failed to set commands for chat %d lang=%s: %s", chat_id, lang, e)


async def refresh_user_commands(
    app_state: object,
    user_id: int,
    role: str,
    lang: str = "en",
    session_factory: object = None,
) -> None:
    """Re-register bot commands for a user after role, language, or permission change.

    Fetches the user's active feature grants from DB when session_factory is
    provided, so feature-gated commands appear/disappear correctly.
    Works only in the combined process (bot in app_state). Silent in API-only mode.
    """
    bot = getattr(app_state, "bot", None)
    if bot is None:
        return

    plugin_commands: list[CommandSpec] = (
        getattr(app_state, "bot_data", {}).get("plugin_commands", [])
    )

    granted_features: set[str] | None = None
    if session_factory is not None:
        try:
            from datetime import datetime, timezone
            from sqlalchemy import select
            from yoink.core.db.models import UserPermission
            from yoink.core.plugin import get_all_features

            now = datetime.now(timezone.utc)
            async with session_factory() as session:
                result = await session.execute(
                    select(UserPermission.plugin, UserPermission.feature).where(
                        UserPermission.user_id == user_id,
                        (UserPermission.expires_at.is_(None)) | (UserPermission.expires_at > now),
                    )
                )
                explicit = {f"{r.plugin}:{r.feature}" for r in result.all()}

            # Add role-based features
            from yoink.core.auth.rbac import ROLE_ORDER
            from yoink.core.db.models import UserRole
            try:
                user_role = UserRole(role)
                role_idx = ROLE_ORDER.index(user_role)
            except ValueError:
                role_idx = 0

            for spec in get_all_features():
                if spec.default_min_role is not None:
                    try:
                        min_idx = ROLE_ORDER.index(UserRole(spec.default_min_role))
                        if role_idx >= min_idx:
                            explicit.add(f"{spec.plugin}:{spec.feature}")
                    except ValueError:
                        pass

            granted_features = explicit
        except Exception as exc:
            logger.warning("Could not load feature grants for user %d: %s", user_id, exc)

    try:
        await set_user_commands(
            bot, user_id, role=role,
            plugin_commands=plugin_commands,
            lang=lang,
            granted_features=granted_features,
        )
        logger.debug("Refreshed commands for user %d (role=%s lang=%s features=%s)", user_id, role, lang, granted_features)
    except Exception as exc:
        logger.warning("Failed to refresh commands for user %d: %s", user_id, exc)
