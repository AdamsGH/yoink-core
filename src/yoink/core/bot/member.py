"""ChatMemberUpdated handler.

Handles two kinds of updates:
- my_chat_member: bot added to / removed from a chat
- chat_member: a user's membership status changed

On join:
  - get_or_create User, upgrade role to group.auto_grant_role if currently lower
  - set BotCommandScopeChatMember for the user in this group
  - apply tag_map permissions if new_member has a tag

On leave / ban:
  - clear BotCommandScopeChatMember scope

On tag change (both statuses in-chat):
  - diff old/new tag, apply/revoke permissions from tag_map
"""
from __future__ import annotations

import json
import logging

from telegram import ChatMemberUpdated, Update
from telegram.ext import ChatMemberHandler, ContextTypes

from yoink.core.db.models import UserRole
from yoink.core.bot.access import ROLE_ORDER

logger = logging.getLogger(__name__)

_JOINED_STATUSES = {"member", "restricted", "administrator", "creator"}
_LEFT_STATUSES = {"left", "kicked"}


def _is_join(update: ChatMemberUpdated) -> bool:
    old = update.old_chat_member.status
    new = update.new_chat_member.status
    return old in _LEFT_STATUSES and new in _JOINED_STATUSES


def _is_leave(update: ChatMemberUpdated) -> bool:
    old = update.old_chat_member.status
    new = update.new_chat_member.status
    return old in _JOINED_STATUSES and new in _LEFT_STATUSES


def _get_tag(member) -> str | None:
    """Extract tag from ChatMemberMember / ChatMemberRestricted (Bot API 9.5)."""
    return getattr(member, "tag", None) or None


async def _apply_tag_permissions(
    user_id: int,
    tag: str | None,
    old_tag: str | None,
    perm_repo,
    bot_settings_repo,
    owner_id: int,
) -> bool:
    """Grant/revoke features based on tag change. Returns True if anything changed."""
    raw = await bot_settings_repo.get("tag_map")
    if not raw:
        return False
    try:
        tag_map: dict[str, list[str]] = json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        return False

    old_features: set[str] = set(tag_map.get(old_tag, [])) if old_tag else set()
    new_features: set[str] = set(tag_map.get(tag, [])) if tag else set()

    to_grant = new_features - old_features
    to_revoke = old_features - new_features

    changed = False
    for feature_key in to_grant:
        parts = feature_key.split(":", 1)
        if len(parts) != 2:
            continue
        plugin, feature = parts
        await perm_repo.grant(user_id, plugin, feature, granted_by=owner_id)
        logger.info("Tag grant user=%d feature=%s (tag=%s)", user_id, feature_key, tag)
        changed = True

    for feature_key in to_revoke:
        parts = feature_key.split(":", 1)
        if len(parts) != 2:
            continue
        plugin, feature = parts
        await perm_repo.revoke(user_id, plugin, feature)
        logger.info("Tag revoke user=%d feature=%s (old_tag=%s)", user_id, feature_key, old_tag)
        changed = True

    return changed


async def handle_my_chat_member(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    """Bot's own membership changed - enable/disable the group record."""
    cmu: ChatMemberUpdated = update.my_chat_member
    if cmu is None:
        return

    chat = cmu.chat
    group_repo = context.bot_data.get("group_repo")
    if group_repo is None:
        return

    new_status = cmu.new_chat_member.status
    if new_status in ("member", "administrator"):
        await group_repo.upsert(
            group_id=chat.id,
            title=chat.title,
            enabled=True,
        )
        logger.info("Bot added to group %d (%s)", chat.id, chat.title)
    elif new_status in ("left", "kicked"):
        await group_repo.update(chat.id, enabled=False)
        logger.info("Bot removed from group %d (%s)", chat.id, chat.title)


async def handle_chat_member(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    """A user's status in the chat changed."""
    cmu: ChatMemberUpdated = update.chat_member
    if cmu is None:
        return

    user_repo = context.bot_data.get("user_repo")
    group_repo = context.bot_data.get("group_repo")
    perm_repo = context.bot_data.get("perm_repo")
    bot_settings_repo = context.bot_data.get("bot_settings_repo")
    plugin_commands: list = context.bot_data.get("plugin_commands", [])

    tg_user = cmu.new_chat_member.user
    chat = cmu.chat
    user_id = tg_user.id
    group_id = chat.id

    from yoink.core.bot.bot_commands import (
        set_member_commands,
        clear_member_commands,
        refresh_user_commands,
        refresh_member_commands,
    )

    if _is_join(cmu):
        if user_repo is None or group_repo is None:
            return

        user = await user_repo.get_or_create(
            user_id,
            username=tg_user.username,
            first_name=tg_user.first_name,
        )

        group = await group_repo.get(group_id)
        if group is None:
            group = await group_repo.upsert(
                group_id=group_id,
                title=chat.title,
                enabled=True,
            )

        # Upgrade role only - never downgrade manually assigned roles.
        if (
            group.auto_grant_role is not None
            and ROLE_ORDER.index(user.role) < ROLE_ORDER.index(group.auto_grant_role)
        ):
            await user_repo.update(user_id, role=group.auto_grant_role)
            user = await user_repo.get_or_create(user_id, username=tg_user.username, first_name=tg_user.first_name)
            logger.info(
                "Auto-granted role %s to user %d in group %d",
                group.auto_grant_role.value, user_id, group_id,
            )

        # Apply tag permissions.
        new_tag = _get_tag(cmu.new_chat_member)
        config = context.bot_data.get("config")
        owner_id = getattr(config, "owner_id", 0) if config else 0
        if perm_repo and bot_settings_repo and new_tag:
            tag_changed = await _apply_tag_permissions(
                user_id, new_tag, None, perm_repo, bot_settings_repo, owner_id
            )
            if tag_changed:
                sf = context.bot_data.get("session_factory")
                await refresh_user_commands(
                    context.application,
                    user_id,
                    role=user.role.value,
                    lang=user.language,
                    session_factory=sf,
                )
                await refresh_member_commands(
                    context.application,
                    user_id,
                    role=user.role.value,
                    lang=user.language,
                    session_factory=sf,
                )

        # Record membership so refresh_member_commands can find this group later.
        try:
            await group_repo.touch_member(group_id=group_id, user_id=user_id)
        except Exception as exc:
            logger.debug("touch_member failed user=%d group=%d: %s", user_id, group_id, exc)

        # Set BotCommandScopeChatMember for this group.
        try:
            await set_member_commands(
                context.bot,
                group_id=group_id,
                user_id=user_id,
                role=user.role.value,
                plugin_commands=plugin_commands,
                lang=user.language,
            )
        except Exception as exc:
            logger.warning("set_member_commands failed user=%d group=%d: %s", user_id, group_id, exc)

    elif _is_leave(cmu):
        try:
            await clear_member_commands(context.bot, group_id=group_id, user_id=user_id)
        except Exception as exc:
            logger.debug("clear_member_commands failed user=%d group=%d: %s", user_id, group_id, exc)

    else:
        # In-chat status change (e.g. tag change, restriction change).
        old_tag = _get_tag(cmu.old_chat_member)
        new_tag = _get_tag(cmu.new_chat_member)
        if old_tag != new_tag and perm_repo and bot_settings_repo:
            config = context.bot_data.get("config")
            owner_id = getattr(config, "owner_id", 0) if config else 0
            user = await user_repo.get_or_create(user_id, username=tg_user.username, first_name=tg_user.first_name) if user_repo else None
            tag_changed = await _apply_tag_permissions(
                user_id, new_tag, old_tag, perm_repo, bot_settings_repo, owner_id
            )
            if tag_changed and user:
                sf = context.bot_data.get("session_factory")
                await refresh_user_commands(
                    context.application,
                    user_id,
                    role=user.role.value,
                    lang=user.language,
                    session_factory=sf,
                )
                await refresh_member_commands(
                    context.application,
                    user_id,
                    role=user.role.value,
                    lang=user.language,
                    session_factory=sf,
                )


def register(app) -> None:
    """Register ChatMemberUpdated handlers on the PTB Application."""
    app.add_handler(ChatMemberHandler(handle_my_chat_member, ChatMemberHandler.MY_CHAT_MEMBER))
    app.add_handler(ChatMemberHandler(handle_chat_member, ChatMemberHandler.CHAT_MEMBER))
