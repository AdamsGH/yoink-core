"""Group management endpoints."""
from __future__ import annotations

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from yoink.core.api.deps import get_db
from yoink.core.api.exceptions import ConflictError, NotFoundError
from yoink.core.api.schemas import GroupResponse, ThreadPolicyInline
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import Group, ThreadPolicy, User, UserGroupPolicy, UserRole


class GroupCreateRequest(BaseModel):
    id: int
    title: str | None = None
    auto_grant_role: UserRole = UserRole.user
    allow_pm: bool = True
    nsfw_allowed: bool = False


class GroupUpdateRequest(BaseModel):
    title: str | None = None
    enabled: bool | None = None
    auto_grant_role: UserRole | None = None
    allow_pm: bool | None = None
    nsfw_allowed: bool | None = None
    storage_chat_id: int | None = None
    storage_thread_id: int | None = None


class ThreadPolicyRequest(BaseModel):
    thread_id: int | None = None
    name: str | None = None
    enabled: bool = True


class ThreadPolicyResponse(BaseModel):
    id: int
    group_id: int
    thread_id: int | None
    name: str | None
    enabled: bool


class UserGroupPolicyRequest(BaseModel):
    role_override: UserRole | None = None
    allow_pm_override: bool | None = None


class UserGroupPolicyResponse(BaseModel):
    user_id: int
    group_id: int
    role_override: UserRole | None
    allow_pm_override: bool | None

router = APIRouter(prefix="/groups", tags=["groups"], responses={401: {"description": "Not authenticated"}, 403: {"description": "Insufficient role"}})


def _thread_policies(group: Group) -> list[ThreadPolicyInline]:
    return [
        ThreadPolicyInline(id=p.id, thread_id=p.thread_id, name=p.name, enabled=p.enabled)
        for p in sorted(group.thread_policies, key=lambda p: p.thread_id or 0)
    ]


def _group_response(group: Group) -> GroupResponse:
    return GroupResponse(
        id=group.id,
        title=group.title,
        enabled=group.enabled,
        auto_grant_role=group.auto_grant_role,
        allow_pm=group.allow_pm,
        nsfw_allowed=group.nsfw_allowed,
        storage_chat_id=group.storage_chat_id,
        storage_thread_id=group.storage_thread_id,
        photo_url=group.photo_url,
        created_at=group.created_at,
        thread_policies=_thread_policies(group),
    )


@router.post("", response_model=GroupResponse, status_code=201, summary="Register a group (admin+)")
async def create_group(
    body: GroupCreateRequest,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> GroupResponse:
    existing = await session.get(Group, body.id)
    if existing is not None:
        raise ConflictError("Group already exists")
    group = Group(
        id=body.id,
        title=body.title,
        auto_grant_role=body.auto_grant_role,
        allow_pm=body.allow_pm,
        nsfw_allowed=body.nsfw_allowed,
    )
    session.add(group)
    await session.commit()
    await session.execute(
        select(Group).options(selectinload(Group.thread_policies)).where(Group.id == group.id)
    )
    await session.refresh(group)
    return _group_response(group)


@router.get("", response_model=list[GroupResponse], summary="List all registered groups (admin+)")
async def list_groups(
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[GroupResponse]:
    result = await session.execute(
        select(Group)
        .options(selectinload(Group.thread_policies))
        .where(Group.id < 0)
        .order_by(Group.id)
    )
    return [_group_response(g) for g in result.scalars().all()]


@router.get("/{group_id}", response_model=GroupResponse, summary="Get group by ID (admin+)")
async def get_group(
    group_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> GroupResponse:
    result = await session.execute(
        select(Group)
        .options(selectinload(Group.thread_policies))
        .where(Group.id == group_id)
    )
    group = result.scalar_one_or_none()
    if group is None:
        raise NotFoundError("Group not found")
    return _group_response(group)


@router.patch("/{group_id}", response_model=GroupResponse, summary="Update group settings (admin+)")
async def update_group(
    group_id: int,
    body: GroupUpdateRequest,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> GroupResponse:
    result = await session.execute(
        select(Group).options(selectinload(Group.thread_policies)).where(Group.id == group_id)
    )
    group = result.scalar_one_or_none()
    if group is None:
        raise NotFoundError("Group not found")
    for field, value in body.model_dump(exclude_none=True).items():
        setattr(group, field, value)
    await session.commit()
    await session.refresh(group)
    return _group_response(group)


@router.delete("/{group_id}", status_code=204, summary="Remove group (admin+)")
async def delete_group(
    group_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> None:
    group = await session.get(Group, group_id)
    if group is None:
        raise NotFoundError("Group not found")
    await session.delete(group)
    await session.commit()


# Thread policies

@router.get("/{group_id}/threads", response_model=list[ThreadPolicyResponse], summary="List thread download policies for group (admin+)")
async def list_thread_policies(
    group_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[ThreadPolicyResponse]:
    if await session.get(Group, group_id) is None:
        raise NotFoundError("Group not found")
    rows = (await session.execute(
        select(ThreadPolicy).where(ThreadPolicy.group_id == group_id).order_by(ThreadPolicy.thread_id)
    )).scalars().all()
    return [ThreadPolicyResponse(
        id=p.id, group_id=p.group_id, thread_id=p.thread_id, name=p.name, enabled=p.enabled,
    ) for p in rows]


@router.post("/{group_id}/threads", response_model=ThreadPolicyResponse, status_code=201, summary="Create thread policy (admin+)")
async def set_thread_policy(
    group_id: int,
    body: ThreadPolicyRequest,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> ThreadPolicyResponse:
    if await session.get(Group, group_id) is None:
        raise NotFoundError("Group not found")
    policy = (await session.execute(
        select(ThreadPolicy).where(
            ThreadPolicy.group_id == group_id,
            ThreadPolicy.thread_id == body.thread_id,
        )
    )).scalar_one_or_none()
    if policy is None:
        policy = ThreadPolicy(
            group_id=group_id, thread_id=body.thread_id, name=body.name, enabled=body.enabled,
        )
        session.add(policy)
    else:
        policy.enabled = body.enabled
        if body.name is not None:
            policy.name = body.name
    await session.commit()
    await session.refresh(policy)
    return ThreadPolicyResponse(
        id=policy.id, group_id=policy.group_id, thread_id=policy.thread_id,
        name=policy.name, enabled=policy.enabled,
    )


@router.delete("/{group_id}/threads/{thread_policy_id}", status_code=204, summary="Delete thread policy (admin+)")
async def delete_thread_policy(
    group_id: int,
    thread_policy_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> None:
    policy = await session.get(ThreadPolicy, thread_policy_id)
    if policy is None or policy.group_id != group_id:
        raise NotFoundError("Thread policy not found")
    await session.delete(policy)
    await session.commit()


# Per-user group overrides

@router.get("/{group_id}/members", response_model=list[UserGroupPolicyResponse], summary="List per-user group overrides (admin+)")
async def list_member_overrides(
    group_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[UserGroupPolicyResponse]:
    if await session.get(Group, group_id) is None:
        raise NotFoundError("Group not found")
    rows = (await session.execute(
        select(UserGroupPolicy).where(UserGroupPolicy.group_id == group_id)
    )).scalars().all()
    return [UserGroupPolicyResponse(
        user_id=p.user_id, group_id=p.group_id,
        role_override=p.role_override, allow_pm_override=p.allow_pm_override,
    ) for p in rows]


@router.put("/{group_id}/members/{user_id}", response_model=UserGroupPolicyResponse, summary="Set per-user group override (admin+)", description="Upsert a `UserGroupPolicy` record. `can_download=null` means inherit group default.")
async def set_member_override(
    group_id: int,
    user_id: int,
    body: UserGroupPolicyRequest,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserGroupPolicyResponse:
    if await session.get(Group, group_id) is None:
        raise NotFoundError("Group not found")
    if await session.get(User, user_id) is None:
        raise NotFoundError("User not found")
    policy = (await session.execute(
        select(UserGroupPolicy).where(
            UserGroupPolicy.user_id == user_id, UserGroupPolicy.group_id == group_id,
        )
    )).scalar_one_or_none()
    if policy is None:
        policy = UserGroupPolicy(
            user_id=user_id, group_id=group_id,
            role_override=body.role_override, allow_pm_override=body.allow_pm_override,
        )
        session.add(policy)
    else:
        policy.role_override = body.role_override
        policy.allow_pm_override = body.allow_pm_override
    await session.commit()
    await session.refresh(policy)
    return UserGroupPolicyResponse(
        user_id=policy.user_id, group_id=policy.group_id,
        role_override=policy.role_override, allow_pm_override=policy.allow_pm_override,
    )


@router.delete("/{group_id}/members/{user_id}", status_code=204, summary="Remove per-user group override (admin+)")
async def remove_member_override(
    group_id: int,
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> None:
    policy = (await session.execute(
        select(UserGroupPolicy).where(
            UserGroupPolicy.user_id == user_id, UserGroupPolicy.group_id == group_id,
        )
    )).scalar_one_or_none()
    if policy is None:
        raise NotFoundError("Override not found")
    await session.delete(policy)
    await session.commit()


from yoink.core.api.photo import bot_api_params, resolve_chat_photo


@router.get("/{group_id}/photo", summary="Proxy group chat photo")
async def get_group_photo(
    group_id: int,
    request: Request,
) -> None:
    """
    Stream the group's Telegram chat photo. No auth required (used in <img src>).
    Always fetches fresh file_id via getChat — survives photo changes.
    """
    from fastapi.responses import Response

    bot_api_url, bot_token = bot_api_params(request.app.state)
    data = await resolve_chat_photo(bot_api_url, bot_token, group_id)
    if not data:
        raise NotFoundError("No photo available")

    return Response(
        content=data,
        media_type="image/jpeg",
        headers={"Cache-Control": "public, max-age=86400"},
    )


@router.post("/photos/sync", summary="Backfill group photos via getChat (owner)")
async def sync_group_photos(
    request: Request,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
):
    """Fetch chat photos for all groups missing them via Bot API getChat."""
    import httpx
    import os

    bot_api_url = os.environ.get("BOT_API_URL", "https://api.telegram.org")
    bot_token = request.app.state.settings.bot_token

    groups_without_photo = (await session.execute(
        select(Group).where(Group.photo_url.is_(None))
    )).scalars().all()

    updated = 0
    for g in groups_without_photo:
        try:
            async with httpx.AsyncClient(timeout=5) as client:
                r = await client.get(f"{bot_api_url}/bot{bot_token}/getChat", params={"chat_id": g.id})
                if r.status_code != 200:
                    continue
                data = r.json()
                if not data.get("ok"):
                    continue
                photo = data["result"].get("photo")
                if photo and photo.get("big_file_id"):
                    g.photo_url = photo["big_file_id"]
                    updated += 1
        except Exception:
            continue

    await session.commit()
    return {"total": len(groups_without_photo), "updated": updated}
