"""
Centralised bot RBAC: AccessPolicy, PermissionChecker, require_access decorator.

Usage in handlers:

    @require_access(AccessPolicy(min_role=UserRole.user))
    async def _cmd_video(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
        ...

    @require_access(AccessPolicy(min_role=UserRole.admin, silent_deny=True))
    async def _cmd_admin(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
        ...

    # Feature-gated: explicit grant OR role >= FeatureSpec.default_min_role
    @require_access(AccessPolicy(plugin="dl", feature="download"))
    async def _cmd_download(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
        ...
"""
from __future__ import annotations

import asyncio
import functools
import logging
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from telegram import Chat, Update
from telegram.ext import ContextTypes

from yoink.core.db.models import UserRole

if TYPE_CHECKING:
    pass

logger = logging.getLogger(__name__)

ROLE_ORDER: list[UserRole] = [
    UserRole.banned,
    UserRole.restricted,
    UserRole.user,
    UserRole.moderator,
    UserRole.admin,
    UserRole.owner,
]


def role_gte(user_role: UserRole, min_role: UserRole) -> bool:
    """True if user_role is at least as permissive as min_role."""
    return ROLE_ORDER.index(user_role) >= ROLE_ORDER.index(min_role)


@dataclass
class AccessPolicy:
    """Declarative access rules for a single bot handler.

    When ``plugin`` and ``feature`` are both set, access is evaluated via
    UserPermissionRepo.has() which checks:
      1. Explicit non-expired grant in user_permissions.
      2. user.role >= FeatureSpec.default_min_role (role threshold).
    The ``min_role`` field still acts as an independent floor that must also
    be satisfied - set it to UserRole.banned to make it a no-op when you only
    want feature-gate logic.
    """

    min_role: UserRole = UserRole.user
    # Allowed chat scopes: "private", "group", "all"
    scopes: list[str] = field(default_factory=lambda: ["all"])
    # Check Group.enabled flag for group messages
    check_group_enabled: bool = True
    # Check ThreadPolicy for group messages
    check_thread_policy: bool = True
    # True  = silently ignore denied messages (no reply)
    # False = send ephemeral error reply
    silent_deny: bool = True
    # When True, override silent_deny=False in group chats - always silent in groups.
    # Useful for feature-gated commands that should reply in private but not spam groups.
    group_silent_deny: bool = False
    # Always log denials at DEBUG level
    log_deny: bool = True
    # Optional feature gate: both fields must be set together
    plugin: str | None = None
    feature: str | None = None


@dataclass
class PermissionResult:
    allowed: bool
    deny_reason: str = ""
    effective_role: UserRole = UserRole.user


class PermissionChecker:
    """
    Stateless permission evaluator.  All DB access goes through bot_data repos
    so it works without a separate database session.
    """

    async def check(
        self,
        user_id: int,
        chat: Chat | None,
        thread_id: int | None,
        policy: AccessPolicy,
        context: ContextTypes.DEFAULT_TYPE,
        username: str | None = None,
        first_name: str | None = None,
    ) -> PermissionResult:
        user_repo = context.bot_data.get("user_repo")
        if user_repo is None:
            return PermissionResult(allowed=False, deny_reason="no_user_repo")

        user = await user_repo.get_or_create(
            user_id, username=username, first_name=first_name,
        )

        is_group = chat is not None and chat.type in ("group", "supergroup")
        is_private = chat is not None and chat.type == "private"

        # Resolve effective role first: a group's auto_grant_role can lift a
        # restricted user before any block/role check is applied.
        effective_role = user.role
        if is_group and chat is not None:
            group_repo = context.bot_data.get("group_repo")
            if group_repo is not None:
                effective_role = await self._resolve_effective_role(
                    user_id, user.role, chat.id, group_repo
                )
                # Persist the grant so future checks (including API) see the upgraded role
                if effective_role != user.role:
                    try:
                        await user_repo.update(user_id, role=effective_role)
                        user.role = effective_role
                    except Exception as exc:
                        logger.debug("Could not persist role grant: %s", exc)

        # Banned users are always blocked regardless of group grants.
        # Restricted users are NOT blocked here - they are handled by the role
        # check below (role_gte will deny them unless a grant elevated them).
        if user.is_blocked:
            return PermissionResult(
                allowed=False,
                deny_reason="blocked",
                effective_role=effective_role,
            )

        # Scope check
        if "all" not in policy.scopes:
            if is_private and "private" not in policy.scopes:
                return PermissionResult(
                    allowed=False,
                    deny_reason="scope_private_not_allowed",
                    effective_role=effective_role,
                )
            if is_group and "group" not in policy.scopes:
                return PermissionResult(
                    allowed=False,
                    deny_reason="scope_group_not_allowed",
                    effective_role=effective_role,
                )

        if not role_gte(effective_role, policy.min_role):
            return PermissionResult(
                allowed=False,
                deny_reason=f"role_{effective_role.value}_below_{policy.min_role.value}",
                effective_role=effective_role,
            )

        # Feature gate: explicit grant OR role threshold from FeatureSpec
        if policy.plugin and policy.feature:
            perm_repo = context.bot_data.get("perm_repo")
            if perm_repo is None:
                return PermissionResult(
                    allowed=False,
                    deny_reason="no_perm_repo",
                    effective_role=effective_role,
                )
            allowed = await perm_repo.has(
                user_id, policy.plugin, policy.feature, user=user
            )
            if not allowed:
                return PermissionResult(
                    allowed=False,
                    deny_reason=f"feature_{policy.plugin}_{policy.feature}_denied",
                    effective_role=effective_role,
                )

        # Group-specific checks
        if is_group and chat is not None:
            group_repo = context.bot_data.get("group_repo")
            if group_repo is not None:
                if policy.check_group_enabled:
                    title = chat.title or str(chat.id)
                    await group_repo.upsert(group_id=chat.id, title=title)
                    if not await group_repo.is_enabled(chat.id):
                        return PermissionResult(
                            allowed=False,
                            deny_reason="group_not_enabled",
                            effective_role=effective_role,
                        )

                if policy.check_thread_policy:
                    allowed = await group_repo.is_thread_allowed(chat.id, thread_id)
                    if not allowed:
                        return PermissionResult(
                            allowed=False,
                            deny_reason="thread_not_allowed",
                            effective_role=effective_role,
                        )

        return PermissionResult(allowed=True, effective_role=effective_role)

    async def _resolve_effective_role(
        self,
        user_id: int,
        global_role: UserRole,
        group_id: int,
        group_repo,
    ) -> UserRole:
        """Return the role that applies in this group.

        Priority:
        1. UserGroupPolicy.role_override (explicit per-user override in group)
        2. Group.auto_grant_role if user's global role is restricted
           (group membership acts as implicit grant)
        3. global_role
        """
        try:
            from sqlalchemy import select
            from yoink.core.db.models import Group, UserGroupPolicy

            async with group_repo._sf() as session:
                result = await session.execute(
                    select(UserGroupPolicy).where(
                        UserGroupPolicy.user_id == user_id,
                        UserGroupPolicy.group_id == group_id,
                    )
                )
                ugp = result.scalar_one_or_none()
                if ugp is not None and ugp.role_override is not None:
                    return ugp.role_override

                # If user is restricted globally, elevate to group's auto_grant_role
                if global_role == UserRole.restricted:
                    group = await session.get(Group, group_id)
                    if group is not None and group.enabled:
                        granted = group.auto_grant_role
                        if ROLE_ORDER.index(granted) > ROLE_ORDER.index(global_role):
                            return granted
        except Exception as exc:
            logger.debug("Could not resolve group role override: %s", exc)
        return global_role


_checker = PermissionChecker()


def require_access(policy: AccessPolicy):
    """
    Decorator for PTB handlers.  Runs PermissionChecker before the handler body.

    Denied access is either silently ignored (policy.silent_deny=True) or
    answered with an ephemeral error reply.
    """
    def decorator(func):
        @functools.wraps(func)
        async def wrapper(update: Update, context: ContextTypes.DEFAULT_TYPE) -> None:
            user = update.effective_user
            if user is None:
                return

            chat = update.effective_chat
            thread_id: int | None = None
            msg = update.effective_message
            if msg:
                thread_id = getattr(msg, "message_thread_id", None)

            result = await _checker.check(
                user_id=user.id,
                chat=chat,
                thread_id=thread_id,
                policy=policy,
                context=context,
                username=user.username,
                first_name=user.first_name,
            )

            if not result.allowed:
                if policy.log_deny:
                    logger.debug(
                        "Access denied: user=%d chat=%s reason=%s handler=%s",
                        user.id,
                        chat.id if chat else "?",
                        result.deny_reason,
                        func.__name__,
                    )
                is_group_ctx = chat is not None and chat.type in ("group", "supergroup")
                effectively_silent = policy.silent_deny or (policy.group_silent_deny and is_group_ctx)
                if not effectively_silent:
                    from yoink.core.bot.middleware import reply_ephemeral
                    from yoink.core.i18n import t
                    lang = "en"
                    try:
                        repo = context.bot_data.get("user_repo")
                        if repo:
                            u = await repo.get_or_create(
                                user.id, username=user.username, first_name=user.first_name,
                            )
                            lang = u.language
                    except Exception:
                        pass
                    await reply_ephemeral(update, context, t("common.access_denied", lang))
                return

            context.user_data["_effective_role"] = result.effective_role

            context.application.create_task(
                _maybe_update_photo(user, context),
                update=update,
            )

            await func(update, context)

        return wrapper
    return decorator


async def _maybe_update_photo(tg_user, context: ContextTypes.DEFAULT_TYPE) -> None:
    """Fetch user profile photo file_id in background if not cached yet."""
    for attempt in range(2):
        try:
            user_repo = context.bot_data.get("user_repo")
            if user_repo is None:
                return
            db_user = await user_repo.get_or_create(tg_user.id)
            if db_user.photo_url:
                return
            photos = await context.bot.get_user_profile_photos(tg_user.id, limit=1)
            if not photos.photos:
                if attempt == 0:
                    await asyncio.sleep(5)
                    continue
                logger.debug(
                    "No profile photos for user %d (total_count=%d)",
                    tg_user.id, photos.total_count,
                )
                return
            photo = photos.photos[0][-1]
            await user_repo.update(tg_user.id, photo_url=photo.file_id)
            logger.debug("Updated photo for user %d: %s", tg_user.id, photo.file_id)
            return
        except Exception:
            logger.warning(
                "_maybe_update_photo attempt %d failed for user %d",
                attempt + 1, tg_user.id, exc_info=True,
            )
