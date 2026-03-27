"""UserPermissionRepo - CRUD for the unified user_permissions table."""
from __future__ import annotations

from datetime import datetime, timezone

from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.db.models import User, UserPermission


class UserPermissionRepo:
    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._sf = session_factory

    async def has(
        self,
        user_id: int,
        plugin: str,
        feature: str,
        user: User | None = None,
    ) -> bool:
        """Return True if the user has access to plugin/feature.

        Two paths are evaluated in order:

        1. Explicit grant: a non-expired row in user_permissions.
        2. Role threshold: user.role >= FeatureSpec.default_min_role,
           provided the user is not blocked and default_min_role is not None.

        Passing ``user`` avoids a second DB round-trip when the caller already
        has the User object (e.g. inside a request that authenticated the user).
        When ``user`` is None, only path (1) is evaluated.

        The owner role sits at the top of the hierarchy so it satisfies any
        non-None default_min_role automatically via role_gte.
        """
        now = datetime.now(timezone.utc)

        # Path 1: explicit grant
        async with self._sf() as s:
            result = await s.execute(
                select(UserPermission.id).where(
                    UserPermission.user_id == user_id,
                    UserPermission.plugin == plugin,
                    UserPermission.feature == feature,
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                )
            )
            if result.scalar_one_or_none() is not None:
                return True

        # Path 2: role threshold (requires caller to supply the User object)
        if user is not None and not user.is_blocked:
            spec = _feature_spec(plugin, feature)
            if spec is not None and spec.default_min_role is not None:
                from yoink.core.bot.access import ROLE_ORDER
                from yoink.core.db.models import UserRole
                try:
                    min_role = UserRole(spec.default_min_role)
                except ValueError:
                    return False
                return ROLE_ORDER.index(user.role) >= ROLE_ORDER.index(min_role)

        return False

    async def list_for_user(self, user_id: int) -> list[UserPermission]:
        now = datetime.now(timezone.utc)
        async with self._sf() as s:
            result = await s.execute(
                select(UserPermission).where(
                    UserPermission.user_id == user_id,
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                ).order_by(UserPermission.plugin, UserPermission.feature)
            )
            return list(result.scalars().all())

    async def list_for_feature(self, plugin: str, feature: str) -> list[UserPermission]:
        now = datetime.now(timezone.utc)
        async with self._sf() as s:
            result = await s.execute(
                select(UserPermission).where(
                    UserPermission.plugin == plugin,
                    UserPermission.feature == feature,
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                ).order_by(UserPermission.granted_at.desc())
            )
            return list(result.scalars().all())

    async def list_all(self) -> list[UserPermission]:
        now = datetime.now(timezone.utc)
        async with self._sf() as s:
            result = await s.execute(
                select(UserPermission).where(
                    (UserPermission.expires_at.is_(None))
                    | (UserPermission.expires_at > now),
                ).order_by(UserPermission.plugin, UserPermission.feature, UserPermission.granted_at.desc())
            )
            return list(result.scalars().all())

    async def grant(
        self,
        user_id: int,
        plugin: str,
        feature: str,
        granted_by: int,
        expires_at: datetime | None = None,
    ) -> UserPermission:
        """Upsert a permission grant. Creates a bare User row if needed."""
        now = datetime.now(timezone.utc)
        async with self._sf() as s:
            user = await s.get(User, user_id)
            if user is None:
                user = User(id=user_id)
                s.add(user)
                await s.flush()

            result = await s.execute(
                select(UserPermission).where(
                    UserPermission.user_id == user_id,
                    UserPermission.plugin == plugin,
                    UserPermission.feature == feature,
                )
            )
            row = result.scalar_one_or_none()
            if row is None:
                row = UserPermission(
                    user_id=user_id,
                    plugin=plugin,
                    feature=feature,
                    granted_by=granted_by,
                    granted_at=now,
                    expires_at=expires_at,
                )
                s.add(row)
            else:
                row.granted_by = granted_by
                row.granted_at = now
                row.expires_at = expires_at
            await s.commit()
            await s.refresh(row)
            return row

    async def revoke(self, user_id: int, plugin: str, feature: str) -> bool:
        async with self._sf() as s:
            result = await s.execute(
                delete(UserPermission).where(
                    UserPermission.user_id == user_id,
                    UserPermission.plugin == plugin,
                    UserPermission.feature == feature,
                )
            )
            await s.commit()
            return result.rowcount > 0

    async def revoke_all_for_user(self, user_id: int) -> int:
        async with self._sf() as s:
            result = await s.execute(
                delete(UserPermission).where(UserPermission.user_id == user_id)
            )
            await s.commit()
            return result.rowcount


def _feature_spec(plugin: str, feature: str):
    """Look up a FeatureSpec from the global registry. Returns None if not found."""
    from yoink.core.plugin import get_all_features
    for spec in get_all_features():
        if spec.plugin == plugin and spec.feature == feature:
            return spec
    return None
