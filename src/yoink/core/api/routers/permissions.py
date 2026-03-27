"""Permission management API endpoints.

GET  /features                                              - list declared features (admin)
GET  /users/{user_id}/feature-access                        - effective access matrix (admin)
GET  /users/{user_id}/permissions                           - list grants for a user (admin)
POST /users/{user_id}/permissions                           - grant a feature (admin)
DELETE /users/{user_id}/permissions/{plugin}/{feature}      - revoke a feature (admin)
"""
from __future__ import annotations

from datetime import datetime, timezone

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.auth.rbac import require_role, ROLE_ORDER
from yoink.core.api.exceptions import NotFoundError
from yoink.core.db.models import User, UserPermission, UserRole
from yoink.core.plugin import get_all_features

router = APIRouter(tags=["permissions"])


class FeatureResponse(BaseModel):
    plugin: str
    feature: str
    label: str
    description: str
    default_min_role: str | None


class PermissionResponse(BaseModel):
    model_config = {"from_attributes": True}

    id: int
    user_id: int
    plugin: str
    feature: str
    granted_by: int
    granted_at: datetime
    expires_at: datetime | None
    grant_source: str = "manual"


class EffectiveFeatureAccess(BaseModel):
    plugin: str
    feature: str
    label: str
    description: str
    default_min_role: str | None
    access_via_role: bool
    access_via_grant: bool
    effective: bool
    grant_expires_at: datetime | None
    grant_source: str = "manual"


class GrantRequest(BaseModel):
    plugin: str
    feature: str
    expires_at: datetime | None = None


@router.get("/features", response_model=list[FeatureResponse])
async def list_features(
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[FeatureResponse]:
    """Return all features declared by loaded plugins."""
    return [
        FeatureResponse(
            plugin=f.plugin,
            feature=f.feature,
            label=f.label,
            description=f.description,
            default_min_role=f.default_min_role,
        )
        for f in get_all_features()
    ]


async def _compute_feature_access(
    session: AsyncSession,
    user: User,
) -> list[EffectiveFeatureAccess]:
    now = datetime.now(timezone.utc)
    result = await session.execute(
        select(UserPermission).where(
            UserPermission.user_id == user.id,
            (UserPermission.expires_at.is_(None)) | (UserPermission.expires_at > now),
        )
    )
    grants: dict[tuple[str, str], UserPermission] = {
        (r.plugin, r.feature): r for r in result.scalars().all()
    }

    rows: list[EffectiveFeatureAccess] = []
    for spec in get_all_features():
        key = (spec.plugin, spec.feature)
        grant = grants.get(key)

        via_role = False
        if user.role == UserRole.owner:
            via_role = True
        elif spec.default_min_role is not None and not user.is_blocked:
            try:
                min_role = UserRole(spec.default_min_role)
                via_role = ROLE_ORDER.index(user.role) >= ROLE_ORDER.index(min_role)
            except ValueError:
                pass

        via_grant = grant is not None
        rows.append(EffectiveFeatureAccess(
            plugin=spec.plugin,
            feature=spec.feature,
            label=spec.label,
            description=spec.description,
            default_min_role=spec.default_min_role,
            access_via_role=via_role,
            access_via_grant=via_grant,
            effective=via_role or via_grant,
            grant_expires_at=grant.expires_at if grant else None,
            grant_source=getattr(grant, "grant_source", "manual") if grant else "manual",
        ))

    return rows


@router.get("/feature-access/me", response_model=list[EffectiveFeatureAccess])
async def get_my_feature_access(
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> list[EffectiveFeatureAccess]:
    """Return effective feature access for the currently authenticated user."""
    return await _compute_feature_access(session, current_user)


@router.get("/users/{user_id}/feature-access", response_model=list[EffectiveFeatureAccess])
async def get_user_feature_access(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[EffectiveFeatureAccess]:
    """Return effective access matrix for a user across all declared features."""
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")
    return await _compute_feature_access(session, user)


@router.get("/permissions/all", response_model=list[PermissionResponse])
async def list_all_permissions(
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[PermissionResponse]:
    """Return all active (non-expired) permissions across all users."""
    now = datetime.now(timezone.utc)
    result = await session.execute(
        select(UserPermission).where(
            (UserPermission.expires_at.is_(None)) | (UserPermission.expires_at > now),
        ).order_by(UserPermission.plugin, UserPermission.feature, UserPermission.granted_at.desc())
    )
    rows = result.scalars().all()
    return [PermissionResponse.model_validate(r) for r in rows]


@router.get("/users/{user_id}/permissions", response_model=list[PermissionResponse])
async def list_user_permissions(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[PermissionResponse]:
    now = datetime.now(timezone.utc)
    result = await session.execute(
        select(UserPermission).where(
            UserPermission.user_id == user_id,
            (UserPermission.expires_at.is_(None)) | (UserPermission.expires_at > now),
        ).order_by(UserPermission.plugin, UserPermission.feature)
    )
    rows = result.scalars().all()
    return [PermissionResponse.model_validate(r) for r in rows]


@router.post("/users/{user_id}/permissions", response_model=PermissionResponse)
async def grant_permission(
    user_id: int,
    body: GrantRequest,
    request: Request,
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> PermissionResponse:
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")

    now = datetime.now(timezone.utc)
    result = await session.execute(
        select(UserPermission).where(
            UserPermission.user_id == user_id,
            UserPermission.plugin == body.plugin,
            UserPermission.feature == body.feature,
        )
    )
    row = result.scalar_one_or_none()
    if row is None:
        row = UserPermission(
            user_id=user_id,
            plugin=body.plugin,
            feature=body.feature,
            granted_by=current_user.id,
            granted_at=now,
            expires_at=body.expires_at,
        )
        session.add(row)
    else:
        row.granted_by = current_user.id
        row.granted_at = now
        row.expires_at = body.expires_at
    await session.commit()
    await session.refresh(row)

    from yoink.core.bot.bot_commands import refresh_user_commands
    sf = getattr(request.app.state, "bot_data", {}).get("session_factory")
    await refresh_user_commands(
        request.app.state, user_id,
        role=user.role.value, lang=user.language,
        session_factory=sf,
    )

    return PermissionResponse.model_validate(row)


@router.delete("/users/{user_id}/permissions/{plugin}/{feature}", response_model=dict)
async def revoke_permission(
    user_id: int,
    plugin: str,
    feature: str,
    request: Request,
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> dict:
    # Tag-sourced grants can only be revoked by the owner, not by admins.
    # They are managed automatically via tag_map and should not be edited manually.
    existing = await session.execute(
        select(UserPermission).where(
            UserPermission.user_id == user_id,
            UserPermission.plugin == plugin,
            UserPermission.feature == feature,
        )
    )
    row = existing.scalar_one_or_none()
    if row is not None and getattr(row, "grant_source", "manual") == "tag":
        if current_user.role != UserRole.owner:
            from yoink.core.api.exceptions import ForbiddenError
            raise ForbiddenError("Tag-sourced grants can only be revoked by the owner")

    user = await session.get(User, user_id)
    result = await session.execute(
        delete(UserPermission).where(
            UserPermission.user_id == user_id,
            UserPermission.plugin == plugin,
            UserPermission.feature == feature,
        )
    )
    await session.commit()

    if user is not None and result.rowcount > 0:
        from yoink.core.bot.bot_commands import refresh_user_commands
        sf = getattr(request.app.state, "bot_data", {}).get("session_factory")
        await refresh_user_commands(
            request.app.state, user_id,
            role=user.role.value, lang=user.language,
            session_factory=sf,
        )

    return {"removed": result.rowcount > 0}
