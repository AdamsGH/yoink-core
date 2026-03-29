"""User management endpoints."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from fastapi import APIRouter, Depends, Query, Request
from sqlalchemy import func, select, text
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.activity import collect_activity
from yoink.core.api.deps import get_current_user, get_db
from yoink.core.api.exceptions import ForbiddenError, NotFoundError
from yoink.core.api.schemas import UserResponse, UserStatsResponse, UserUpdateRequest
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole

router = APIRouter(
    prefix="/users",
    tags=["users"],
    responses={401: {"description": "Not authenticated"}, 403: {"description": "Insufficient role"}},
)

_SORT_FIELDS = {"created_at", "updated_at", "name", "role", "dl_count", "dl_last_at"}

_MUSIC_DOMAINS = frozenset({
    "open.spotify.com", "spotify.com",
    "music.yandex.ru", "music.yandex.com",
    "deezer.com", "www.deezer.com",
    "music.apple.com", "soundcloud.com", "music.youtube.com",
})
_VIDEO_DOMAINS = frozenset({
    "youtube.com", "youtu.be", "m.youtube.com", "www.youtube.com",
    "tiktok.com", "vimeo.com", "twitch.tv",
    "instagram.com", "ig.me", "twitter.com", "x.com",
    "reddit.com", "redd.it",
})

_LIST_USERS_SQL = """
    SELECT
        u.id, u.username, u.first_name, u.photo_url,
        u.role, u.language, u.theme, u.ban_until,
        u.created_at, u.updated_at,
        COALESCE(dl.dl_count, 0) AS dl_count,
        dl.dl_last_at
    FROM users u
    LEFT JOIN LATERAL (
        SELECT COUNT(*) AS dl_count, MAX(created_at) AS dl_last_at
        FROM download_log WHERE user_id = u.id
    ) dl ON true
    {where}
    ORDER BY {order}
    LIMIT :limit OFFSET :offset
"""

_COUNT_USERS_SQL = "SELECT COUNT(*) FROM users u {where}"


def _categorize_domain(domain: str | None) -> str:
    if not domain:
        return "other"
    d = domain.lower().removeprefix("www.")
    if d in _MUSIC_DOMAINS:
        return "music"
    if d in _VIDEO_DOMAINS:
        return "video"
    return "other"


def _build_users_where(search: str | None, role: str | None, status: str | None) -> tuple[str, dict]:
    clauses: list[str] = []
    params: dict = {}
    if search:
        s = search.lstrip("@")
        clauses.append(
            "(u.username ILIKE :search OR u.first_name ILIKE :search OR CAST(u.id AS TEXT) = :search_exact)"
        )
        params["search"] = f"%{s}%"
        params["search_exact"] = s
    if role and role != "all":
        clauses.append("u.role = :role")
        params["role"] = role
    if status == "active":
        clauses.append("u.role NOT IN ('restricted', 'banned')")
    elif status == "restricted":
        clauses.append("u.role = 'restricted'")
    elif status == "banned":
        clauses.append("u.role = 'banned'")
    where = ("WHERE " + " AND ".join(clauses)) if clauses else ""
    return where, params


def _build_users_order(sort: str, direction: str) -> str:
    desc = "DESC" if direction == "desc" else "ASC"
    nulls = "NULLS LAST" if desc == "DESC" else "NULLS FIRST"
    match sort:
        case "name":
            return f"COALESCE(u.first_name, u.username, CAST(u.id AS TEXT)) {desc}"
        case "role":
            return f"u.role {desc}"
        case "dl_count":
            return f"dl_count {desc} {nulls}"
        case "dl_last_at":
            return f"dl_last_at {desc} {nulls}"
        case "updated_at":
            return f"u.updated_at {desc}"
        case _:
            return f"u.created_at {desc}"


def _user_response(u: User, dl_count: int = 0, dl_last_at: datetime | None = None) -> UserResponse:
    return UserResponse(
        id=u.id, username=u.username, first_name=u.first_name, photo_url=u.photo_url,
        role=u.role, language=u.language, theme=u.theme,
        ban_until=u.ban_until, created_at=u.created_at, updated_at=u.updated_at,
        dl_count=dl_count, dl_last_at=dl_last_at,
    )


@router.get("/me/stats", response_model=UserStatsResponse, summary="My activity statistics")
async def get_my_stats(
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> UserStatsResponse:
    return await _build_user_stats(session, current_user.id, current_user.created_at)


@router.get("/me", response_model=UserResponse, summary="Current user profile")
async def get_me(current_user: User = Depends(get_current_user)) -> UserResponse:
    return _user_response(current_user)


@router.get("", response_model=dict, summary="List all users (admin+)")
async def list_users(
    offset: int = Query(0, ge=0),
    limit: int = Query(50, ge=1, le=200),
    search: str | None = Query(None),
    role: str | None = Query(None),
    status: str | None = Query(None),
    sort: str = Query("created_at"),
    direction: str = Query("desc"),
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> dict:
    if sort not in _SORT_FIELDS:
        sort = "created_at"
    if direction not in ("asc", "desc"):
        direction = "desc"

    try:
        from yoink_dl.storage.models import DownloadLog as _DL  # noqa: F401, PLC0415
        has_dl = True
    except ImportError:
        has_dl = False

    where, params = _build_users_where(search, role, status)

    if has_dl:
        order = _build_users_order(sort, direction)
        total = (await session.execute(text(_COUNT_USERS_SQL.format(where=where)), params)).scalar_one()
        rows = (await session.execute(
            text(_LIST_USERS_SQL.format(where=where, order=order)),
            {**params, "limit": limit, "offset": offset},
        )).fetchall()
        items = [UserResponse(
            id=r.id, username=r.username, first_name=r.first_name, photo_url=r.photo_url,
            role=r.role, language=r.language, theme=r.theme,
            ban_until=r.ban_until, created_at=r.created_at, updated_at=r.updated_at,
            dl_count=r.dl_count, dl_last_at=r.dl_last_at,
        ) for r in rows]
    else:
        q = select(User)
        if search:
            s = search.lstrip("@")
            q = q.where(User.username.ilike(f"%{s}%") | User.first_name.ilike(f"%{s}%"))
        if role and role != "all":
            try:
                q = q.where(User.role == UserRole(role))
            except ValueError:
                pass
        if status == "active":
            q = q.where(User.role.notin_([UserRole.restricted, UserRole.banned]))
        elif status == "restricted":
            q = q.where(User.role == UserRole.restricted)
        elif status == "banned":
            q = q.where(User.role == UserRole.banned)
        total = (await session.execute(select(func.count()).select_from(q.subquery()))).scalar_one()
        users = (await session.execute(q.order_by(User.created_at.desc()).offset(offset).limit(limit))).scalars().all()
        items = [_user_response(u) for u in users]

    return {"items": items, "total": total, "offset": offset, "limit": limit}


@router.get("/{user_id}/stats", response_model=UserStatsResponse, summary="Activity statistics for any user (admin+)")
async def get_user_stats(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserStatsResponse:
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")
    return await _build_user_stats(session, user_id, user.created_at)


@router.get("/{user_id}", response_model=UserResponse, summary="Get user by ID (admin+)")
async def get_user(
    user_id: int,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> UserResponse:
    user = await session.get(User, user_id)
    if user is None:
        raise NotFoundError("User not found")
    return _user_response(user)


@router.patch("/{user_id}", response_model=UserResponse, summary="Update user role / language / ban (admin+)", description="Changing role triggers refresh_member_commands for all groups where the bot is present.")
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
        if body.role == UserRole.owner:
            raise ForbiddenError("Owner role cannot be assigned via API")
        if current_user.role == UserRole.admin and user.role in (UserRole.admin, UserRole.owner):
            raise ForbiddenError("Admins cannot change role of admin or owner accounts")
        if user.role != body.role:
            user.role = body.role
            role_changed = True

    if body.ban_until is not None:
        user.ban_until = body.ban_until
    elif "ban_until" in (body.model_fields_set or set()):
        user.ban_until = None

    if body.language is not None:
        from yoink.core.i18n.loader import SUPPORTED  # noqa: PLC0415
        if body.language in SUPPORTED:
            user.language = body.language

    await session.commit()
    await session.refresh(user)

    if role_changed or body.language is not None:
        from yoink.core.bot.bot_commands import refresh_member_commands, refresh_user_commands  # noqa: PLC0415
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

    return _user_response(user)


async def _build_user_stats(session: AsyncSession, user_id: int, member_since: datetime) -> UserStatsResponse:
    """Aggregate activity statistics from all registered plugin providers."""
    now = datetime.now(timezone.utc)
    today_start = now.replace(hour=0, minute=0, second=0, microsecond=0)
    week_start = today_start - timedelta(days=7)

    total = today_count = week_count = 0
    top_domains: list[dict] = []
    by_category: dict[str, int] = {}
    dl_last_at: datetime | None = None
    music_total = 0
    music_last_at: datetime | None = None
    ai_total = 0
    ai_last_at: datetime | None = None

    plugin_items = await collect_activity(session, user_id)

    for item in plugin_items:
        plugin = item.get("plugin", "")
        extra = item.get("extra", {})

        if plugin == "dl":
            total = item.get("total", 0)
            today_count = extra.get("today", 0)
            week_count = extra.get("this_week", 0)
            top_domains = extra.get("top_domains", [])
            by_category = extra.get("by_category", {})
            dl_last_at = item.get("last_at")
        elif plugin == "music":
            music_total = item.get("total", 0)
            music_last_at = item.get("last_at")
        elif plugin == "insight":
            ai_total = item.get("total", 0)
            ai_last_at = item.get("last_at")

    return UserStatsResponse(
        total=total,
        today=today_count,
        this_week=week_count,
        top_domains=top_domains,
        member_since=member_since,
        by_category=by_category,
        dl_last_at=dl_last_at,
        music_total=music_total,
        music_last_at=music_last_at,
        ai_total=ai_total,
        ai_last_at=ai_last_at,
        plugins=plugin_items,  # type: ignore[arg-type]
    )
