"""User management endpoints."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.api.exceptions import ForbiddenError, NotFoundError
from yoink.core.api.schemas import UserResponse, UserUpdateRequest
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole

router = APIRouter(prefix="/users", tags=["users"], responses={401: {"description": "Not authenticated"}, 403: {"description": "Insufficient role"}})


class UserStatsResponse(BaseModel):
    total: int
    this_week: int
    today: int
    top_domains: list[dict]
    member_since: datetime
    # Breakdown by category - present only when dl plugin is loaded
    by_category: dict[str, int] = {}


_MUSIC_DOMAINS = frozenset({
    "open.spotify.com", "spotify.com",
    "music.yandex.ru", "music.yandex.com",
    "deezer.com", "www.deezer.com",
    "music.apple.com",
    "soundcloud.com",
    "music.youtube.com",
})
_VIDEO_DOMAINS = frozenset({
    "youtube.com", "youtu.be", "m.youtube.com", "www.youtube.com",
    "tiktok.com", "vimeo.com", "twitch.tv",
    "instagram.com", "ig.me",
    "twitter.com", "x.com",
    "reddit.com", "redd.it",
})


def _categorize_domain(domain: str | None) -> str:
    if not domain:
        return "other"
    d = domain.lower().removeprefix("www.")
    if d in _MUSIC_DOMAINS:
        return "music"
    if d in _VIDEO_DOMAINS:
        return "video"
    return "other"


@router.get("/me/stats", response_model=UserStatsResponse, summary="My download statistics")
async def get_my_stats(
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> UserStatsResponse:
    """Personal download statistics - requires yoink-dl DownloadLog table."""
    from sqlalchemy import text  # noqa: PLC0415
    now = datetime.now(timezone.utc)
    today_start = now.replace(hour=0, minute=0, second=0, microsecond=0)
    week_start = today_start - timedelta(days=7)

    try:
        from yoink_dl.storage.models import DownloadLog  # noqa: PLC0415

        base = DownloadLog.user_id == current_user.id
        total = (await session.execute(
            select(func.count()).select_from(DownloadLog).where(base)
        )).scalar_one()
        today_count = (await session.execute(
            select(func.count()).select_from(DownloadLog)
            .where(base, DownloadLog.created_at >= today_start)
        )).scalar_one()
        week_count = (await session.execute(
            select(func.count()).select_from(DownloadLog)
            .where(base, DownloadLog.created_at >= week_start)
        )).scalar_one()
        top_result = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
            .order_by(func.count().desc())
            .limit(5)
        )
        top_domains = [{"domain": r.domain, "count": r.cnt} for r in top_result]
        # Per-category counts
        cat_result = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
        )
        by_category: dict[str, int] = {"video": 0, "music": 0, "other": 0}
        for r in cat_result:
            by_category[_categorize_domain(r.domain)] += r.cnt
    except ImportError:
        total = today_count = week_count = 0
        top_domains = []
        by_category = {}

    return UserStatsResponse(
        total=total,
        today=today_count,
        this_week=week_count,
        top_domains=top_domains,
        member_since=current_user.created_at,
        by_category=by_category,
    )


@router.get("/me", response_model=UserResponse, summary="Current user profile")
async def get_me(current_user: User = Depends(get_current_user)) -> UserResponse:
    return UserResponse(
        id=current_user.id,
        username=current_user.username,
        first_name=current_user.first_name,
        role=current_user.role,
        language=current_user.language,
        theme=current_user.theme,
        ban_until=current_user.ban_until,
        created_at=current_user.created_at,
        updated_at=current_user.updated_at,
    )


@router.get("", response_model=dict, summary="List all users (admin+)", description="Paginated user list with optional search by username and filter by role/status.")
async def list_users(
    offset: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
    search: str | None = Query(None),
    role: str | None = Query(None),
    status: str | None = Query(None),
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> dict:
    q = select(User)
    if search:
        q = q.where(
            User.username.ilike(f"%{search.lstrip('@')}%")
            | func.cast(User.id, type_=User.id.type).in_(
                [search] if search.isdigit() else []
            )
        )
    if role and role != 'all':
        try:
            q = q.where(User.role == UserRole(role))
        except ValueError:
            pass
    if status == 'active':
        q = q.where(User.role.notin_([UserRole.restricted, UserRole.banned]))
    elif status == 'restricted':
        q = q.where(User.role == UserRole.restricted)
    elif status == 'banned':
        q = q.where(User.role == UserRole.banned)
    total = (await session.execute(select(func.count()).select_from(q.subquery()))).scalar_one()
    result = await session.execute(q.order_by(User.created_at.desc()).offset(offset).limit(limit))
    users = result.scalars().all()
    return {
        "items": [UserResponse(
            id=u.id, username=u.username, first_name=u.first_name,
            role=u.role, language=u.language, theme=u.theme,
            ban_until=u.ban_until, created_at=u.created_at, updated_at=u.updated_at,
        ) for u in users],
        "total": total,
        "offset": offset,
        "limit": limit,
    }


@router.get("/{user_id}/stats", response_model=UserStatsResponse, summary="Download statistics for any user (admin+)")
async def get_user_stats(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserStatsResponse:
    """Download statistics for any user (admin only)."""
    now = datetime.now(timezone.utc)
    today_start = now.replace(hour=0, minute=0, second=0, microsecond=0)
    week_start = today_start - timedelta(days=7)

    try:
        from yoink_dl.storage.models import DownloadLog  # noqa: PLC0415

        base = DownloadLog.user_id == user_id
        total = (await session.execute(
            select(func.count()).select_from(DownloadLog).where(base)
        )).scalar_one()
        today_count = (await session.execute(
            select(func.count()).select_from(DownloadLog)
            .where(base, DownloadLog.created_at >= today_start)
        )).scalar_one()
        week_count = (await session.execute(
            select(func.count()).select_from(DownloadLog)
            .where(base, DownloadLog.created_at >= week_start)
        )).scalar_one()
        top_result = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
            .order_by(func.count().desc())
            .limit(5)
        )
        top_domains = [{"domain": r.domain, "count": r.cnt} for r in top_result]
        cat_result2 = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
        )
        by_category: dict[str, int] = {"video": 0, "music": 0, "other": 0}
        for r in cat_result2:
            by_category[_categorize_domain(r.domain)] += r.cnt
        user = await session.get(User, user_id)
    except ImportError:
        total = today_count = week_count = 0
        top_domains = []
        by_category = {}
        user = await session.get(User, user_id)

    if user is None:
        raise NotFoundError("User not found")

    return UserStatsResponse(
        total=total,
        today=today_count,
        this_week=week_count,
        top_domains=top_domains,
        member_since=user.created_at,
        by_category=by_category,
    )


@router.get("/{user_id}", response_model=UserResponse, summary="Get user by ID (admin+)")
async def get_user(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserResponse:
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")
    return UserResponse(
        id=user.id, username=user.username, first_name=user.first_name,
        role=user.role, language=user.language, theme=user.theme,
        ban_until=user.ban_until, created_at=user.created_at, updated_at=user.updated_at,
    )


@router.patch("/{user_id}", response_model=UserResponse, summary="Update user role / language / ban (admin+)", description="Changing role triggers `refresh_member_commands` for all groups where the bot is present.")
async def update_user(
    user_id: int,
    body: UserUpdateRequest,
    request: Request,
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserResponse:
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")

    role_changed = False
    if body.role is not None:
        # Assigning owner role via API is never permitted - owner is config-only
        if body.role == UserRole.owner:
            raise ForbiddenError("Owner role cannot be assigned via API")
        # Admins cannot change role of other admins or owners
        if current_user.role == UserRole.admin and user.role in (UserRole.admin, UserRole.owner):
            raise ForbiddenError("Admins cannot change role of admin or owner accounts")
        if user.role != body.role:
            user.role = body.role
            role_changed = True

    if body.ban_until is not None:
        user.ban_until = body.ban_until
    elif body.ban_until is None and "ban_until" in (body.model_fields_set or set()):
        user.ban_until = None

    if body.language is not None:
        from yoink.core.i18n.loader import SUPPORTED
        if body.language in SUPPORTED:
            user.language = body.language

    await session.commit()
    await session.refresh(user)

    if role_changed or body.language is not None:
        from yoink.core.bot.bot_commands import refresh_user_commands, refresh_member_commands
        sf = getattr(request.app.state, "bot_data", {}).get("session_factory")
        await refresh_user_commands(
            request.app.state, user_id,
            role=user.role.value, lang=user.language,
            session_factory=sf,
        )
        if role_changed:
            await refresh_member_commands(
                request.app.state, user_id,
                role=user.role.value, lang=user.language,
                session_factory=sf,
            )

    return UserResponse(
        id=user.id, username=user.username, first_name=user.first_name,
        role=user.role, language=user.language, theme=user.theme,
        ban_until=user.ban_until, created_at=user.created_at, updated_at=user.updated_at,
    )



