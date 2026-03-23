"""Role-based access control helpers."""
from __future__ import annotations

from collections.abc import Callable

from fastapi import Depends, HTTPException, status

from yoink.core.db.models import User, UserRole


ROLE_ORDER = [
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


def require_role(min_role: UserRole, *_extra: UserRole) -> Callable:
    """FastAPI dependency: raise 403 if user role is below min_role.

    Extra role arguments are accepted for backwards compatibility but ignored -
    min_role is the floor and role_gte handles the hierarchy.
    """
    async def _check(user: User = Depends(_get_current_user_dep())) -> User:
        if not role_gte(user.role, min_role):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"Requires role >= {min_role.value}",
            )
        return user
    return _check


def _get_current_user_dep():
    from yoink.core.api.deps import get_current_user
    return get_current_user
