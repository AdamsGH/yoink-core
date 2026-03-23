"""M2M (Machine-to-Machine) internal API endpoints.

All routes require X-Api-Key authentication with scope checks.
Mounted under /api/internal/v1/.
"""
from __future__ import annotations

from datetime import datetime, timezone

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps_m2m import require_scope, _get_db
from yoink.core.db.models import ApiKey, Event, Group, User, UserRole

router = APIRouter(tags=["internal"])


class StatusResponse(BaseModel):
    bot_running: bool
    bot_username: str | None
    uptime_seconds: float
    total_users: int
    total_groups: int
    active_groups: int


class UserM2MResponse(BaseModel):
    id: int
    username: str | None
    first_name: str | None
    role: str
    language: str
    is_blocked: bool
    created_at: datetime


class GroupM2MResponse(BaseModel):
    id: int
    title: str | None
    enabled: bool
    auto_grant_role: str
    created_at: datetime


class EventCreateRequest(BaseModel):
    plugin: str
    event_type: str
    user_id: int | None = None
    group_id: int | None = None
    thread_id: int | None = None
    payload: dict | None = None


class EventResponse(BaseModel):
    id: int
    plugin: str
    event_type: str
    user_id: int | None
    group_id: int | None
    payload: dict | None
    created_at: datetime


@router.get("/status", response_model=StatusResponse)
async def get_status(
    request: Request,
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("health:read")),
) -> StatusResponse:
    from yoink.core.metrics import metrics

    total_users = (await session.execute(
        select(func.count()).select_from(User)
    )).scalar_one()
    total_groups = (await session.execute(
        select(func.count()).select_from(Group)
    )).scalar_one()
    active_groups = (await session.execute(
        select(func.count()).select_from(Group).where(Group.enabled.is_(True))
    )).scalar_one()

    bot = getattr(request.app.state, "bot", None)
    bot_running = bot is not None
    bot_username = None
    if bot:
        me = await bot.get_me()
        bot_username = f"@{me.username}" if me.username else str(me.id)

    snap = metrics.snapshot()

    return StatusResponse(
        bot_running=bot_running,
        bot_username=bot_username,
        uptime_seconds=snap.get("uptime_seconds", 0),
        total_users=total_users,
        total_groups=total_groups,
        active_groups=active_groups,
    )


@router.get("/users", response_model=list[UserM2MResponse])
async def list_users(
    offset: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
    role: str | None = Query(None),
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("users:read")),
) -> list[UserM2MResponse]:
    q = select(User)
    if role:
        try:
            q = q.where(User.role == UserRole(role))
        except ValueError:
            pass
    result = await session.execute(
        q.order_by(User.created_at.desc()).offset(offset).limit(limit)
    )
    return [
        UserM2MResponse(
            id=u.id,
            username=u.username,
            first_name=u.first_name,
            role=u.role.value,
            language=u.language,
            is_blocked=u.is_blocked,
            created_at=u.created_at,
        )
        for u in result.scalars().all()
    ]


@router.get("/users/{user_id}", response_model=UserM2MResponse)
async def get_user(
    user_id: int,
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("users:read")),
) -> UserM2MResponse:
    user = await session.get(User, user_id)
    if user is None:
        from fastapi import HTTPException
        raise HTTPException(status_code=404, detail="User not found")
    return UserM2MResponse(
        id=user.id,
        username=user.username,
        first_name=user.first_name,
        role=user.role.value,
        language=user.language,
        is_blocked=user.is_blocked,
        created_at=user.created_at,
    )


@router.get("/groups", response_model=list[GroupM2MResponse])
async def list_groups(
    enabled_only: bool = Query(False),
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("groups:read")),
) -> list[GroupM2MResponse]:
    q = select(Group)
    if enabled_only:
        q = q.where(Group.enabled.is_(True))
    result = await session.execute(q.order_by(Group.id))
    return [
        GroupM2MResponse(
            id=g.id,
            title=g.title,
            enabled=g.enabled,
            auto_grant_role=g.auto_grant_role.value,
            created_at=g.created_at,
        )
        for g in result.scalars().all()
    ]


@router.post("/events", response_model=EventResponse, status_code=201)
async def create_event(
    body: EventCreateRequest,
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("events:write")),
) -> EventResponse:
    event = Event(
        plugin=body.plugin,
        event_type=body.event_type,
        user_id=body.user_id,
        group_id=body.group_id,
        thread_id=body.thread_id,
        payload=body.payload,
    )
    session.add(event)
    await session.commit()
    await session.refresh(event)
    return EventResponse(
        id=event.id,
        plugin=event.plugin,
        event_type=event.event_type,
        user_id=event.user_id,
        group_id=event.group_id,
        payload=event.payload,
        created_at=event.created_at,
    )


@router.get("/events", response_model=list[EventResponse])
async def list_events(
    offset: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
    plugin: str | None = Query(None),
    event_type: str | None = Query(None),
    session: AsyncSession = Depends(_get_db),
    _key: ApiKey = Depends(require_scope("events:read")),
) -> list[EventResponse]:
    q = select(Event)
    if plugin:
        q = q.where(Event.plugin == plugin)
    if event_type:
        q = q.where(Event.event_type == event_type)
    result = await session.execute(
        q.order_by(Event.created_at.desc()).offset(offset).limit(limit)
    )
    return [
        EventResponse(
            id=e.id,
            plugin=e.plugin,
            event_type=e.event_type,
            user_id=e.user_id,
            group_id=e.group_id,
            payload=e.payload,
            created_at=e.created_at,
        )
        for e in result.scalars().all()
    ]
