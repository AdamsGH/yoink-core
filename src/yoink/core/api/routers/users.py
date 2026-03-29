"""User management endpoints."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel
from sqlalchemy import func, select, text, literal_column
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
    by_category: dict[str, int] = {}
    dl_last_at: datetime | None = None
    music_total: int = 0
    music_last_at: datetime | None = None
    ai_total: int = 0
    ai_last_at: datetime | None = None


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

    # Include music resolves (from yoink-music plugin) in the music category
    try:
        from yoink_music.storage.models import MusicResolveLog  # noqa: PLC0415
        music_count = (await session.execute(
            select(func.count()).select_from(MusicResolveLog)
            .where(MusicResolveLog.user_id == current_user.id)
        )).scalar_one()
        if music_count:
            by_category.setdefault("music", 0)
            by_category["music"] += music_count
            total += music_count
            music_today = (await session.execute(
                select(func.count()).select_from(MusicResolveLog)
                .where(MusicResolveLog.user_id == current_user.id, MusicResolveLog.created_at >= today_start)
            )).scalar_one()
            today_count += music_today
            music_week = (await session.execute(
                select(func.count()).select_from(MusicResolveLog)
                .where(MusicResolveLog.user_id == current_user.id, MusicResolveLog.created_at >= week_start)
            )).scalar_one()
            week_count += music_week
    except ImportError:
        pass

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


_SORT_FIELDS = {"created_at", "updated_at", "name", "role", "dl_count", "dl_last_at"}
_ROLE_ORDER = {"owner": 0, "admin": 1, "moderator": 2, "user": 3, "restricted": 4, "banned": 5}

_LIST_USERS_SQL = """
    SELECT
        u.id, u.username, u.first_name, u.photo_url,
        u.role, u.language, u.theme, u.ban_until,
        u.created_at, u.updated_at,
        COALESCE(dl.dl_count, 0)  AS dl_count,
        dl.dl_last_at
    FROM users u
    LEFT JOIN LATERAL (
        SELECT COUNT(*) AS dl_count, MAX(created_at) AS dl_last_at
        FROM download_log
        WHERE user_id = u.id
    ) dl ON true
    {where}
    ORDER BY {order}
    LIMIT :limit OFFSET :offset
"""

_COUNT_USERS_SQL = """
    SELECT COUNT(*) FROM users u {where}
"""


def _build_users_where(search: str | None, role: str | None, status: str | None) -> tuple[str, dict]:
    clauses: list[str] = []
    params: dict = {}
    if search:
        s = search.lstrip("@")
        clauses.append("(u.username ILIKE :search OR u.first_name ILIKE :search OR CAST(u.id AS TEXT) = :search_exact)")
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
    if sort == "name":
        return f"COALESCE(u.first_name, u.username, CAST(u.id AS TEXT)) {desc}"
    if sort == "role":
        return f"u.role {desc}"
    if sort == "dl_count":
        return f"dl_count {desc} {nulls}"
    if sort == "dl_last_at":
        return f"dl_last_at {desc} {nulls}"
    if sort == "updated_at":
        return f"u.updated_at {desc}"
    return f"u.created_at {desc}"


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
        count_sql = _COUNT_USERS_SQL.format(where=where)
        list_sql = _LIST_USERS_SQL.format(where=where, order=order)
        total = (await session.execute(text(count_sql), params)).scalar_one()
        rows = (await session.execute(text(list_sql), {**params, "limit": limit, "offset": offset})).fetchall()
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
        result = await session.execute(q.order_by(User.created_at.desc()).offset(offset).limit(limit))
        users = result.scalars().all()
        items = [UserResponse(
            id=u.id, username=u.username, first_name=u.first_name, photo_url=u.photo_url,
            role=u.role, language=u.language, theme=u.theme,
            ban_until=u.ban_until, created_at=u.created_at, updated_at=u.updated_at,
        ) for u in users]

    return {"items": items, "total": total, "offset": offset, "limit": limit}


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

    total = today_count = week_count = 0
    top_domains: list[dict] = []
    by_category: dict[str, int] = {}
    dl_last_at: datetime | None = None
    music_total = 0
    music_last_at: datetime | None = None
    ai_total = 0
    ai_last_at: datetime | None = None

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
        dl_last_row = (await session.execute(
            select(func.max(DownloadLog.created_at)).where(base)
        )).scalar_one()
        dl_last_at = dl_last_row
        top_result = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
            .order_by(func.count().desc())
            .limit(5)
        )
        top_domains = [{"domain": r.domain, "count": r.cnt} for r in top_result]
        cat_result = await session.execute(
            select(DownloadLog.domain, func.count().label("cnt"))
            .where(base, DownloadLog.domain.isnot(None))
            .group_by(DownloadLog.domain)
        )
        by_category = {"video": 0, "music": 0, "other": 0}
        for r in cat_result:
            by_category[_categorize_domain(r.domain)] += r.cnt
    except ImportError:
        pass

    try:
        from yoink_music.storage.models import MusicResolveLog  # noqa: PLC0415

        mbase = MusicResolveLog.user_id == user_id
        music_total = (await session.execute(
            select(func.count()).select_from(MusicResolveLog).where(mbase)
        )).scalar_one()
        music_last_at = (await session.execute(
            select(func.max(MusicResolveLog.created_at)).where(mbase)
        )).scalar_one()
    except ImportError:
        pass

    try:
        from yoink_insight.storage.models import InsightUsageLog  # noqa: PLC0415

        abase = InsightUsageLog.user_id == user_id
        ai_total = (await session.execute(
            select(func.count()).select_from(InsightUsageLog).where(abase)
        )).scalar_one()
        ai_last_at = (await session.execute(
            select(func.max(InsightUsageLog.created_at)).where(abase)
        )).scalar_one()
    except ImportError:
        pass

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
        dl_last_at=dl_last_at,
        music_total=music_total,
        music_last_at=music_last_at,
        ai_total=ai_total,
        ai_last_at=ai_last_at,
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
        id=user.id, username=user.username, first_name=user.first_name, photo_url=user.photo_url,
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
        id=user.id, username=user.username, first_name=user.first_name, photo_url=user.photo_url,
        role=user.role, language=user.language, theme=user.theme,
        ban_until=user.ban_until, created_at=user.created_at, updated_at=user.updated_at,
    )




@router.post("/photos/sync", summary="Backfill user photos via getChat (owner)")
async def sync_user_photos(
    request: Request,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
):
    """Fetch profile photos for all users missing them via Bot API getChat."""
    import httpx
    import os

    bot_api_url = os.environ.get("BOT_API_URL", "https://api.telegram.org")
    bot_token = request.app.state.settings.bot_token

    users_without_photo = (await session.execute(
        select(User).where(User.photo_url.is_(None))
    )).scalars().all()

    updated = 0
    for u in users_without_photo:
        try:
            async with httpx.AsyncClient(timeout=5) as client:
                r = await client.get(f"{bot_api_url}/bot{bot_token}/getChat", params={"chat_id": u.id})
                if r.status_code != 200:
                    continue
                data = r.json()
                if not data.get("ok"):
                    continue
                photo = data["result"].get("photo")
                if photo and photo.get("big_file_id"):
                    u.photo_url = photo["big_file_id"]
                    updated += 1
        except Exception:
            continue

    await session.commit()
    return {"total": len(users_without_photo), "updated": updated}


from yoink.core.api.photo import bot_api_params, resolve_chat_photo


@router.get("/{user_id}/photo", summary="Proxy user profile photo")
async def get_user_photo(
    user_id: int,
    request: Request,
):
    """
    Stream user's current Telegram profile photo.
    Always calls getChat for a fresh file_id — survives photo changes.
    No DB lookup needed: photo is fetched live from Bot API.
    """
    from fastapi.responses import Response

    bot_api_url, bot_token = bot_api_params(request.app.state)
    data = await resolve_chat_photo(bot_api_url, bot_token, user_id)
    if not data:
        raise NotFoundError("No photo available")

    return Response(
        content=data,
        media_type="image/jpeg",
        headers={"Cache-Control": "public, max-age=86400"},
    )
