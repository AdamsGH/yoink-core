"""Effective feature grants - single source of truth for "does this user have feature X?".

A user can hold a `plugin:feature` grant via several independent sources:

1. Explicit row in `user_permissions` (non-expired).
2. Role threshold from `FeatureSpec.default_min_role`.
3. Plugin-registered feature providers (e.g. yoink-insight BYOK: if the global
   toggle is on and the user has a probed key, that counts as an `insight:tldr`
   grant even without a row in `user_permissions`).

The owner role is treated as universal: every registered feature is effective
for the owner regardless of provider state.

All three callsites that previously hand-rolled this collection
(`_cmd_help`, `refresh_user_commands`, `refresh_member_commands`,
`PermissionChecker.check`) go through `EffectiveFeatureResolver`.

Provider protocol
-----------------
A provider is an async callable:

    async (user_id: int, session_factory, bot_data) -> bool

It returns True if the user has the feature via that provider's source.
Providers must never raise; exceptions are logged and treated as False.
"""
from __future__ import annotations

import logging
from collections.abc import Awaitable, Callable
from enum import StrEnum
from typing import TYPE_CHECKING, Any

from sqlalchemy import select

from yoink.core.auth.rbac import ROLE_ORDER
from yoink.core.db.models import User, UserPermission, UserRole
from yoink.core.plugin import get_all_features

if TYPE_CHECKING:
    from datetime import datetime

    from sqlalchemy.ext.asyncio import async_sessionmaker

logger = logging.getLogger(__name__)


FeatureProvider = Callable[[int, "async_sessionmaker", dict], Awaitable[bool]]


class GrantSource(StrEnum):
    """Why a user holds a plugin:feature grant.

    Callers that gate UI on "is this a real RBAC grant or a side-channel
    provider grant?" use grant_source() and compare against these values.
    `owner` short-circuits everything; `explicit` is a row in
    user_permissions; `role` is the default_min_role threshold; `provider`
    is any registered FeatureProvider (e.g. BYOK readiness for insight:tldr).
    """
    owner = "owner"
    explicit = "explicit"
    role = "role"
    provider = "provider"


# key = "plugin:feature", value = list of providers
_providers: dict[str, list[FeatureProvider]] = {}


def register_feature_provider(plugin: str, feature: str, provider: FeatureProvider) -> None:
    """Register an additional grant source for plugin:feature. Idempotent per callable."""
    key = f"{plugin}:{feature}"
    bucket = _providers.setdefault(key, [])
    if provider not in bucket:
        bucket.append(provider)
        logger.debug("Feature provider registered: %s", key)


def _providers_for(plugin: str, feature: str) -> list[FeatureProvider]:
    return _providers.get(f"{plugin}:{feature}", [])


class EffectiveFeatureResolver:
    """Resolves effective feature grants for a user.

    Stateless; safe to instantiate once and share, or instantiate per-call.
    All DB access goes through the supplied async_sessionmaker; provider calls
    additionally receive the live bot_data dict for plugin-specific lookups.
    """

    def __init__(self, session_factory: async_sessionmaker, bot_data: dict | None = None) -> None:
        self._sf = session_factory
        self._bot_data = bot_data or {}

    async def is_allowed(
        self,
        user_id: int,
        plugin: str,
        feature: str,
        user: Any | None = None,
    ) -> bool:
        """Return True if the user has plugin:feature by any source.

        Thin convenience wrapper around grant_source() for callers that
        don't care WHY the grant exists.
        """
        return await self.grant_source(user_id, plugin, feature, user=user) is not None

    async def grant_source(
        self,
        user_id: int,
        plugin: str,
        feature: str,
        user: Any | None = None,
    ) -> GrantSource | None:
        """Return the source of a plugin:feature grant, or None if not granted.

        Order: owner -> explicit grant -> role threshold -> registered
        providers. First match wins. Callers that need to distinguish
        "real RBAC grant" from "provider-side grant" (e.g. UI that gates
        gateway-only controls) use this directly. The plain boolean
        is_allowed() wraps this.
        """
        from datetime import UTC, datetime

        now = datetime.now(UTC)

        if user is None:
            user = await self._load_user(user_id)

        # Owner short-circuits: every registered feature is effective.
        if user is not None and not user.is_blocked and user.role == UserRole.owner:
            return GrantSource.owner

        async with self._sf() as s:
            row = await s.execute(
                select(UserPermission.id).where(
                    UserPermission.user_id == user_id,
                    UserPermission.plugin == plugin,
                    UserPermission.feature == feature,
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                )
            )
            if row.scalar_one_or_none() is not None:
                return GrantSource.explicit

        if user is not None and not user.is_blocked:
            spec = _feature_spec(plugin, feature)
            if spec is not None and spec.default_min_role is not None:
                try:
                    min_role = UserRole(spec.default_min_role)
                except ValueError:
                    min_role = None
                if min_role is not None and ROLE_ORDER.index(user.role) >= ROLE_ORDER.index(min_role):
                    return GrantSource.role

        for provider in _providers_for(plugin, feature):
            try:
                if await provider(user_id, self._sf, self._bot_data):
                    return GrantSource.provider
            except Exception:
                logger.exception(
                    "Feature provider raised for %s:%s user=%d", plugin, feature, user_id,
                )
        return None

    async def resolve(
        self,
        user_id: int,
        user: User | None = None,
        role: UserRole | str | None = None,
    ) -> set[str]:
        """Return the full set of effective "plugin:feature" grants for the user.

        Accepts either a User object, a UserRole, or neither (in which case the
        user is loaded from DB). The owner role short-circuits to "every
        registered feature is effective" without consulting providers.
        """
        granted: set[str] = set()

        # Path 1: explicit grants
        granted.update(await self._explicit_grants(user_id))

        # Resolve role: prefer explicit user object, then provided role, then DB.
        effective_role = self._resolve_role(user, role)
        if effective_role is None and user is None:
            user = await self._load_user(user_id)
            if user is not None:
                effective_role = user.role

        is_owner = effective_role == UserRole.owner
        is_blocked = user.is_blocked if user is not None else False

        all_features = get_all_features()

        # Path 2: role threshold + owner short-circuit
        if not is_blocked and effective_role is not None:
            role_idx = ROLE_ORDER.index(effective_role)
            for spec in all_features:
                key = f"{spec.plugin}:{spec.feature}"
                if key in granted:
                    continue
                if is_owner:
                    granted.add(key)
                    continue
                if spec.default_min_role is None:
                    continue
                try:
                    min_idx = ROLE_ORDER.index(UserRole(spec.default_min_role))
                except ValueError:
                    continue
                if role_idx >= min_idx:
                    granted.add(key)

        # Path 3: registered providers (skip for owner; already covered)
        if not is_owner and not is_blocked:
            for spec in all_features:
                key = f"{spec.plugin}:{spec.feature}"
                if key in granted:
                    continue
                for provider in _providers_for(spec.plugin, spec.feature):
                    try:
                        if await provider(user_id, self._sf, self._bot_data):
                            granted.add(key)
                            break
                    except Exception:
                        logger.exception(
                            "Feature provider raised for %s user=%d", key, user_id,
                        )

        return granted

    async def _explicit_grants(self, user_id: int) -> set[str]:
        from datetime import UTC, datetime
        now: datetime = datetime.now(UTC)
        async with self._sf() as s:
            result = await s.execute(
                select(UserPermission.plugin, UserPermission.feature).where(
                    UserPermission.user_id == user_id,
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                )
            )
            return {f"{r.plugin}:{r.feature}" for r in result.all()}

    async def _load_user(self, user_id: int) -> User | None:
        async with self._sf() as s:
            return await s.get(User, user_id)

    def _resolve_role(self, user: User | None, role: UserRole | str | None) -> UserRole | None:
        if user is not None:
            return user.role
        if role is None:
            return None
        if isinstance(role, UserRole):
            return role
        try:
            return UserRole(role)
        except ValueError:
            return None


def _feature_spec(plugin: str, feature: str):
    for spec in get_all_features():
        if spec.plugin == plugin and spec.feature == feature:
            return spec
    return None
