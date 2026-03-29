"""
/group and /thread admin commands for group/thread access control.

/group info                         - show current group config
/group enable                       - enable bot in this group
/group disable                      - disable bot in this group (silences it)
/group allow_pm <on|off>            - toggle PM access for group members
/group nsfw <on|off>                - allow or block NSFW content in this group
/group role <role>                  - set auto_grant_role for new members
/thread allow [thread_id]           - allow this thread (default: current)
/thread deny [thread_id]            - deny this thread (default: current)
/thread list                        - list all thread policies for this group
/thread reset [thread_id]           - remove policy (revert to group default)
"""
from __future__ import annotations

import logging

from telegram import Update
from telegram.ext import Application, CommandHandler, ContextTypes, filters

from yoink.core.bot.middleware import guard_admin
from yoink.core.db.models import UserRole
from yoink.core.db.repos.groups import GroupRepo
from yoink.core.i18n import t

logger = logging.getLogger(__name__)

_VALID_ROLES = {r.value for r in UserRole}


def _get_group_repo(context: ContextTypes.DEFAULT_TYPE) -> GroupRepo | None:
    return context.bot_data.get("group_repo")


def _current_thread_id(update: Update) -> int | None:
    msg = update.message
    if msg and msg.is_topic_message:
        return msg.message_thread_id
    return None


async def _lang(update: Update, context: ContextTypes.DEFAULT_TYPE) -> str:
    if update.effective_user:
        user_repo = context.bot_data.get("user_repo")
        if user_repo:
            user = await user_repo.get_or_create(update.effective_user.id)
            return user.language
    return "en"


def _chat_photo_url(chat) -> str | None:
    """Extract big_file_id from a Chat object if available."""
    try:
        return chat.photo.big_file_id if chat.photo else None
    except Exception:
        return None


async def _cmd_group(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_chat:
        return
    if not await guard_admin(update, context):
        return

    lang = await _lang(update, context)
    chat = update.effective_chat
    if chat.type not in ("group", "supergroup"):
        await update.message.reply_text(t("group.only_groups", lang))
        return

    repo = _get_group_repo(context)
    if repo is None:
        await update.message.reply_text(t("group.not_available", lang))
        return

    args = context.args or []
    sub = args[0].lower() if args else "info"
    group_id = chat.id

    if sub == "info":
        group = await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))
        status = t("common.enabled", lang) if group.enabled else t("common.disabled", lang)
        nsfw_status = t("group.nsfw_on", lang) if group.nsfw_allowed else t("group.nsfw_off", lang)
        await update.message.reply_html(
            t("group.info", lang,
              title=group.title or str(group_id),
              status=status,
              role=group.auto_grant_role.value,
              allow_pm=group.allow_pm,
              nsfw_status=nsfw_status),
        )

    elif sub in ("enable", "disable"):
        val = sub == "enable"
        await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))
        await repo.update(group_id=group_id, enabled=val)
        msg = t("group.enabled", lang) if val else t("group.disabled", lang)
        await update.message.reply_text(msg)

    elif sub == "allow_pm":
        if len(args) < 2 or args[1].lower() not in ("on", "off"):
            await update.message.reply_text(t("group.usage_allow_pm", lang))
            return
        val = args[1].lower() == "on"
        await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))
        await repo.update(group_id=group_id, allow_pm=val)
        state = t("common.enabled", lang) if val else t("common.disabled", lang)
        await update.message.reply_text(t("group.allow_pm_changed", lang, state=state.lower()))

    elif sub == "nsfw":
        if len(args) < 2 or args[1].lower() not in ("on", "off"):
            await update.message.reply_text(t("group.usage_nsfw", lang))
            return
        val = args[1].lower() == "on"
        await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))
        await repo.update(group_id=group_id, nsfw_allowed=val)
        await update.message.reply_text(t("group.nsfw_on" if val else "group.nsfw_off", lang))

    elif sub == "role":
        if len(args) < 2 or args[1].lower() not in _VALID_ROLES:
            roles = ", ".join(_VALID_ROLES)
            await update.message.reply_text(t("group.usage_role", lang, roles=roles))
            return
        role = UserRole(args[1].lower())
        await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))
        await repo.update(group_id=group_id, auto_grant_role=role)
        await update.message.reply_html(t("group.role_set", lang, role=role.value))

    else:
        await update.message.reply_text(t("group.usage", lang))


async def _cmd_thread(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
    if not update.message or not update.effective_chat:
        return
    if not await guard_admin(update, context):
        return

    lang = await _lang(update, context)
    chat = update.effective_chat
    if chat.type not in ("group", "supergroup"):
        await update.message.reply_text(t("group.only_groups", lang))
        return

    repo = _get_group_repo(context)
    if repo is None:
        await update.message.reply_text(t("group.not_available", lang))
        return

    args = context.args or []
    sub = args[0].lower() if args else "list"
    group_id = chat.id

    await repo.upsert(group_id=group_id, title=chat.title, photo_url=_chat_photo_url(chat))

    if sub == "list":
        policies = await repo.list_thread_policies(group_id)
        if not policies:
            await update.message.reply_text(t("group.thread_no_policies", lang))
            return
        lines = []
        for p in policies:
            tid = p.thread_id if p.thread_id is not None else "main"
            label = f"{p.name} (<code>{tid}</code>)" if p.name else f"<code>{tid}</code>"
            state = "✅ allowed" if p.enabled else "🚫 denied"
            lines.append(f"{label}: {state}")
        await update.message.reply_html("\n".join(lines))

    elif sub in ("allow", "deny"):
        enabled = sub == "allow"
        thread_id: int | None = None
        if len(args) >= 2:
            try:
                thread_id = int(args[1])
            except ValueError:
                await update.message.reply_text(t("group.thread_invalid_id", lang))
                return
        else:
            thread_id = _current_thread_id(update)
        await repo.set_thread_policy(group_id=group_id, thread_id=thread_id, enabled=enabled)
        tid_label = str(thread_id) if thread_id is not None else "main"
        state = "allowed" if enabled else "denied"
        await update.message.reply_html(t("group.thread_state", lang, tid=tid_label, state=state))

    elif sub == "reset":
        thread_id = None
        if len(args) >= 2:
            try:
                thread_id = int(args[1])
            except ValueError:
                await update.message.reply_text(t("group.thread_invalid_id", lang))
                return
        else:
            thread_id = _current_thread_id(update)
        removed = await repo.delete_thread_policy(group_id=group_id, thread_id=thread_id)
        tid_label = str(thread_id) if thread_id is not None else "main"
        key = "group.thread_policy_removed" if removed else "group.thread_policy_not_found"
        await update.message.reply_html(t(key, lang, tid=tid_label))

    else:
        await update.message.reply_text(t("group.thread_usage", lang))


def register(app: Application) -> None:
    app.add_handler(CommandHandler("group", _cmd_group, filters=filters.ChatType.GROUPS))
    app.add_handler(CommandHandler("thread", _cmd_thread, filters=filters.ChatType.GROUPS))
