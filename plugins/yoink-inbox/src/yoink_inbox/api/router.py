"""Inbox FastAPI router (mounted at /api/v1/inbox by yoink-core).

Endpoints:

  GET    /health                       liveness
  GET    /items                        cursor-paginated inbox list
  POST   /items                        ingest a URL from web/extension
  GET    /items/{id}                   single item with categories
  DELETE /items/{id}                   archive (soft delete via archived_at)
  POST   /items/{id}/reclassify        re-run classify on demand
  GET    /categories                   list user's categories + counts
  GET    /gh_stars                     cursor-paginated starred repos
  POST   /gh_stars/sync                enqueue stars sync

All endpoints require Bearer auth + an effective `inbox:ingest` grant
(via EffectiveFeatureResolver, so role threshold and provider hooks both
count). Cursor format is opaque base64url of `created_at|id` so the order
ties cleanly into the `(user_id, status, created_at, id)` composite index.
"""
from __future__ import annotations

import base64
import logging
import re
from datetime import datetime
from typing import TYPE_CHECKING

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import and_, delete as sql_delete, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.auth.effective_features import EffectiveFeatureResolver
from yoink.core.db.models import User, UserRole
from yoink_inbox.api.schemas import (
    InboxCategoryCreate,
    InboxCategoryRead,
    InboxCategoryUpdate,
    InboxGhFolderCreate,
    InboxGhFolderRead,
    InboxGhStarListResponse,
    InboxGhStarRead,
    InboxItemCategoryRef,
    InboxItemCreate,
    InboxItemListResponse,
    InboxItemRead,
    InboxRuleCreate,
    InboxRuleRead,
    InboxRuleTestResult,
    InboxRuleUpdate,
    InboxTeamCreate,
    InboxTeamMemberRead,
    InboxTeamMemberUpsert,
    InboxTeamRead,
)
from yoink_inbox.storage.models import (
    InboxCategory,
    InboxGhFolder,
    InboxGhFolderMember,
    InboxGhStar,
    InboxGhSyncState,
    InboxItem,
    InboxItemCategory,
    InboxRule,
    InboxTeam,
    InboxTeamMember,
)

if TYPE_CHECKING:
    pass

logger = logging.getLogger(__name__)


def _slugify(name: str) -> str:
    return re.sub(r'[^a-z0-9]+', '-', name.lower()).strip('-')[:64]


async def _unique_slug_category(
    session: AsyncSession, user_id: int, base: str, exclude_id: int | None = None
) -> str:
    slug = base or 'category'
    for suffix in ['', *[f'-{i}' for i in range(2, 20)]]:
        candidate = (slug + suffix)[:64]
        stmt = select(InboxCategory.id).where(
            InboxCategory.owner_user_id == user_id,
            InboxCategory.slug == candidate,
        )
        if exclude_id is not None:
            stmt = stmt.where(InboxCategory.id != exclude_id)
        if not await session.scalar(stmt):
            return candidate
    return slug + '-x'


async def _unique_slug_folder(
    session: AsyncSession, user_id: int, base: str, exclude_id: int | None = None
) -> str:
    slug = base or 'folder'
    for suffix in ['', *[f'-{i}' for i in range(2, 20)]]:
        candidate = (slug + suffix)[:64]
        stmt = select(InboxGhFolder.id).where(
            InboxGhFolder.user_id == user_id,
            InboxGhFolder.slug == candidate,
        )
        if exclude_id is not None:
            stmt = stmt.where(InboxGhFolder.id != exclude_id)
        if not await session.scalar(stmt):
            return candidate
    return slug + '-x'


async def _unique_slug_team(
    session: AsyncSession, user_id: int, base: str, exclude_id: int | None = None
) -> str:
    slug = base or 'team'
    for suffix in ['', *[f'-{i}' for i in range(2, 20)]]:
        candidate = (slug + suffix)[:64]
        stmt = select(InboxTeam.id).where(
            InboxTeam.owner_user_id == user_id,
            InboxTeam.slug == candidate,
        )
        if exclude_id is not None:
            stmt = stmt.where(InboxTeam.id != exclude_id)
        if not await session.scalar(stmt):
            return candidate
    return slug + '-x'


# NOTE: no prefix here. yoink-core mounts this router at /api/v1/inbox via
# `app.include_router(router, prefix=f"/api/v1/{plugin.name}")`. A prefix here
# would double up to /api/v1/inbox/inbox.
router = APIRouter(tags=["inbox"])


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _is_owner(user: User) -> bool:
    return user.role == UserRole.owner


def _resolver_for(request: Request) -> EffectiveFeatureResolver:
    sf = request.app.state.session_factory
    bot_data = getattr(request.app.state, "bot_data", {}) or {}
    return EffectiveFeatureResolver(sf, bot_data)


async def _require_feature(request: Request, user: User, feature: str) -> None:
    """Raise 403 unless the user has `inbox:<feature>` effectively granted."""
    if _is_owner(user):
        return
    resolver = _resolver_for(request)
    ok = await resolver.is_allowed(user.id, "inbox", feature, user=user)
    if not ok:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"inbox:{feature} not granted",
        )


def _encode_cursor(created_at: datetime, row_id: int) -> str:
    """Opaque base64url cursor on (created_at_iso, id).

    Keeps the wire format opaque so we can change the underlying ordering
    later without rolling clients.
    """
    raw = f"{created_at.isoformat()}|{row_id}".encode()
    return base64.urlsafe_b64encode(raw).decode().rstrip("=")


def _decode_cursor(cursor: str | None) -> tuple[datetime, int] | None:
    if not cursor:
        return None
    try:
        # Pad back to a multiple of 4 for b64 strictness.
        padded = cursor + "=" * (-len(cursor) % 4)
        raw = base64.urlsafe_b64decode(padded.encode()).decode()
        iso, sid = raw.split("|", 1)
        return datetime.fromisoformat(iso), int(sid)
    except (ValueError, UnicodeDecodeError) as exc:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid cursor",
        ) from exc


async def _load_categories_for_items(
    session: AsyncSession, item_ids: list[int]
) -> dict[int, list[InboxItemCategoryRef]]:
    """Bulk-load category bindings for a page of items.

    One JOIN instead of an N+1 storm; keys returned dict by item_id so the
    caller can splice them into the response.
    """
    if not item_ids:
        return {}
    rows = (
        await session.execute(
            select(InboxItemCategory, InboxCategory)
            .join(InboxCategory, InboxCategory.id == InboxItemCategory.category_id)
            .where(InboxItemCategory.item_id.in_(item_ids))
        )
    ).all()
    out: dict[int, list[InboxItemCategoryRef]] = {}
    for binding, category in rows:
        out.setdefault(binding.item_id, []).append(
            InboxItemCategoryRef(
                id=category.id,
                name=category.name,
                slug=category.slug,
                color=category.color,
                attached_by=binding.attached_by,
                confidence=binding.confidence,
            )
        )
    return out


# ---------------------------------------------------------------------------
# Health
# ---------------------------------------------------------------------------


@router.get("/health")
async def health() -> dict[str, str]:
    """Liveness probe used by `just verify` smoke tests."""
    return {"status": "ok", "plugin": "inbox"}


# ---------------------------------------------------------------------------
# Items
# ---------------------------------------------------------------------------


@router.get("/items", response_model=InboxItemListResponse)
async def list_items(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
    cursor: str | None = Query(default=None),
    limit: int = Query(default=30, ge=1, le=100),
    status_filter: str | None = Query(default=None, alias="status"),
    kind: str | None = Query(default=None),
    category_id: int | None = Query(default=None),
    search: str | None = Query(default=None, min_length=2, max_length=128),
) -> InboxItemListResponse:
    """Paginated inbox feed for the authenticated user.

    Sort order: created_at DESC, id DESC (stable tiebreak). Cursor encodes
    the last row's (created_at, id), so we get keyset pagination over the
    composite index without the LIMIT/OFFSET drift problem on busy feeds.
    """
    await _require_feature(request, user, "ingest")

    stmt = select(InboxItem).where(InboxItem.user_id == user.id)
    if status_filter:
        stmt = stmt.where(InboxItem.status == status_filter)
    else:
        # Default view hides archived; explicit ?status=archived to see them.
        stmt = stmt.where(InboxItem.archived_at.is_(None))
    if kind:
        stmt = stmt.where(InboxItem.kind == kind)
    if search:
        # Cheap ILIKE for now; switch to PG FTS once we have a tsvector column.
        pat = f"%{search}%"
        stmt = stmt.where(
            or_(
                InboxItem.title.ilike(pat),
                InboxItem.summary.ilike(pat),
                InboxItem.url.ilike(pat),
            )
        )
    if category_id is not None:
        stmt = stmt.join(
            InboxItemCategory, InboxItemCategory.item_id == InboxItem.id
        ).where(InboxItemCategory.category_id == category_id)

    after = _decode_cursor(cursor)
    if after is not None:
        after_dt, after_id = after
        stmt = stmt.where(
            or_(
                InboxItem.created_at < after_dt,
                and_(InboxItem.created_at == after_dt, InboxItem.id < after_id),
            )
        )

    stmt = stmt.order_by(InboxItem.created_at.desc(), InboxItem.id.desc()).limit(
        limit + 1
    )
    rows = list((await session.execute(stmt)).scalars().all())

    next_cursor: str | None = None
    if len(rows) > limit:
        rows = rows[:limit]
        last = rows[-1]
        next_cursor = _encode_cursor(last.created_at, last.id)

    cat_map = await _load_categories_for_items(session, [r.id for r in rows])
    items = [
        InboxItemRead.model_validate(
            {**r.__dict__, "categories": cat_map.get(r.id, [])}
        )
        for r in rows
    ]
    return InboxItemListResponse(items=items, next_cursor=next_cursor)


@router.post(
    "/items", response_model=InboxItemRead, status_code=status.HTTP_201_CREATED
)
async def create_item(
    request: Request,
    payload: InboxItemCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxItemRead:
    """Ingest a URL submitted from the web UI or a browser extension."""
    await _require_feature(request, user, "ingest")

    from yoink_inbox.services.ingest import ingest_url

    bot_data = getattr(request.app.state, "bot_data", {}) or {}
    arq = bot_data.get("inbox_arq_pool")

    res = await ingest_url(
        session,
        user_id=user.id,
        url=str(payload.url),
        source=payload.source,
        arq=arq,
    )
    await session.commit()

    item = await session.get(InboxItem, res.item_id)
    if item is None:
        # Should not happen unless somebody soft-deleted mid-call.
        raise HTTPException(status_code=500, detail="Item missing post-insert")

    cat_map = await _load_categories_for_items(session, [item.id])
    return InboxItemRead.model_validate(
        {**item.__dict__, "categories": cat_map.get(item.id, [])}
    )


@router.get("/items/{item_id}", response_model=InboxItemRead)
async def get_item(
    request: Request,
    item_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxItemRead:
    await _require_feature(request, user, "ingest")
    item = await session.get(InboxItem, item_id)
    if item is None or item.user_id != user.id:
        raise HTTPException(status_code=404, detail="Item not found")
    cat_map = await _load_categories_for_items(session, [item.id])
    return InboxItemRead.model_validate(
        {**item.__dict__, "categories": cat_map.get(item.id, [])}
    )


@router.delete("/items/{item_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_item(
    request: Request,
    item_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
    hard: bool = Query(
        default=False,
        description="Hard-delete the row instead of archiving it.",
    ),
) -> None:
    """Soft-delete by default; pass `?hard=true` to wipe the row.

    Soft-delete sets archived_at and flips status to 'archived', so the
    item disappears from the default feed but stays in DB for category
    statistics. Hard-delete is a separate code path so the UI can offer
    a confirmation flow before destroying audit history.
    """
    await _require_feature(request, user, "ingest")
    item = await session.get(InboxItem, item_id)
    if item is None or item.user_id != user.id:
        raise HTTPException(status_code=404, detail="Item not found")

    if hard:
        await session.execute(
            sql_delete(InboxItem).where(InboxItem.id == item_id)
        )
    else:
        item.status = "archived"
        item.archived_at = datetime.now(item.created_at.tzinfo)
    await session.commit()


@router.post("/items/{item_id}/reclassify", status_code=status.HTTP_202_ACCEPTED)
async def reclassify_item(
    request: Request,
    item_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> dict[str, str]:
    """Enqueue a classify pass; falls back to inline run if Redis is down."""
    await _require_feature(request, user, "classify")
    item = await session.get(InboxItem, item_id)
    if item is None or item.user_id != user.id:
        raise HTTPException(status_code=404, detail="Item not found")

    bot_data = getattr(request.app.state, "bot_data", {}) or {}
    arq = bot_data.get("inbox_arq_pool")
    if arq is not None:
        await arq.enqueue_job(
            "classify_item", item_id, _queue_name="inbox:default"
        )
        return {"status": "queued"}

    # Inline fallback.
    sf = request.app.state.session_factory
    from yoink_inbox.services.classify import run_classify

    await run_classify(sf, item_id)
    return {"status": "done"}


# ---------------------------------------------------------------------------
# Categories
# ---------------------------------------------------------------------------


@router.get("/categories", response_model=list[InboxCategoryRead])
async def list_categories(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> list[InboxCategoryRead]:
    """List the user's own categories + categories shared with their teams.

    Counts are computed in the same query (LEFT JOIN inbox_item_categories)
    so the UI can render badge counts without N+1 SELECTs.
    """
    await _require_feature(request, user, "ingest")

    # Step 1: collect team_ids the user belongs to.
    team_ids = (
        await session.execute(
            select(InboxTeamMember.team_id).where(InboxTeamMember.user_id == user.id)
        )
    ).scalars().all()

    if team_ids:
        visibility = or_(
            InboxCategory.owner_user_id == user.id,
            InboxCategory.shared_with_team_id.in_(team_ids),
        )
    else:
        visibility = InboxCategory.owner_user_id == user.id

    # Subquery: per-category item count restricted to items the caller owns.
    # A user who is a team-member sees the shared category but its count
    # still reflects only the items they ingested (matches the per-user
    # inbox view; team-wide stats are a separate endpoint we don't ship yet).
    count_sq = (
        select(
            InboxItemCategory.category_id,
            func.count(InboxItem.id).label("cnt"),
        )
        .join(InboxItem, InboxItem.id == InboxItemCategory.item_id)
        .where(InboxItem.user_id == user.id, InboxItem.archived_at.is_(None))
        .group_by(InboxItemCategory.category_id)
        .subquery()
    )

    stmt = (
        select(InboxCategory, func.coalesce(count_sq.c.cnt, 0))
        .outerjoin(count_sq, count_sq.c.category_id == InboxCategory.id)
        .where(visibility)
        .order_by(InboxCategory.name.asc())
    )
    rows = (await session.execute(stmt)).all()
    return [
        InboxCategoryRead.model_validate({**cat.__dict__, "item_count": int(cnt)})
        for cat, cnt in rows
    ]


# ---------------------------------------------------------------------------
# Categories CRUD
# ---------------------------------------------------------------------------


@router.post("/categories", response_model=InboxCategoryRead, status_code=status.HTTP_201_CREATED)
async def create_category(
    request: Request,
    payload: InboxCategoryCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxCategoryRead:
    await _require_feature(request, user, "ingest")
    slug = payload.slug or _slugify(payload.name)
    slug = await _unique_slug_category(session, user.id, slug)
    cat = InboxCategory(
        owner_user_id=user.id,
        name=payload.name,
        slug=slug,
        icon=payload.icon,
        color=payload.color,
        description=payload.description,
        parent_id=payload.parent_id,
        shared_with_team_id=payload.shared_with_team_id,
        kind="user",
    )
    session.add(cat)
    await session.commit()
    await session.refresh(cat)
    return InboxCategoryRead.model_validate({**cat.__dict__, "item_count": 0})


@router.get("/categories/{cat_id}", response_model=InboxCategoryRead)
async def get_category(
    request: Request,
    cat_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxCategoryRead:
    await _require_feature(request, user, "ingest")
    cat = await session.get(InboxCategory, cat_id)
    if cat is None or cat.owner_user_id != user.id:
        raise HTTPException(status_code=404, detail="Category not found")
    cnt = await session.scalar(
        select(func.count(InboxItemCategory.item_id))
        .where(InboxItemCategory.category_id == cat_id)
    ) or 0
    return InboxCategoryRead.model_validate({**cat.__dict__, "item_count": cnt})


@router.put("/categories/{cat_id}", response_model=InboxCategoryRead)
async def update_category(
    request: Request,
    cat_id: int,
    payload: InboxCategoryUpdate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxCategoryRead:
    await _require_feature(request, user, "ingest")
    cat = await session.get(InboxCategory, cat_id)
    if cat is None or cat.owner_user_id != user.id:
        raise HTTPException(status_code=404, detail="Category not found")
    slug = payload.slug or _slugify(payload.name)
    if slug != cat.slug:
        slug = await _unique_slug_category(session, user.id, slug, exclude_id=cat_id)
    cat.name = payload.name
    cat.slug = slug
    cat.icon = payload.icon
    cat.color = payload.color
    cat.description = payload.description
    cat.parent_id = payload.parent_id
    cat.shared_with_team_id = payload.shared_with_team_id
    await session.commit()
    await session.refresh(cat)
    cnt = await session.scalar(
        select(func.count(InboxItemCategory.item_id))
        .where(InboxItemCategory.category_id == cat_id)
    ) or 0
    return InboxCategoryRead.model_validate({**cat.__dict__, "item_count": cnt})


@router.delete("/categories/{cat_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_category(
    request: Request,
    cat_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "ingest")
    cat = await session.get(InboxCategory, cat_id)
    if cat is None or cat.owner_user_id != user.id:
        raise HTTPException(status_code=404, detail="Category not found")
    await session.delete(cat)
    await session.commit()


# ---------------------------------------------------------------------------
# GH Folders CRUD
# ---------------------------------------------------------------------------


@router.get("/folders", response_model=list[InboxGhFolderRead])
async def list_folders(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> list[InboxGhFolderRead]:
    await _require_feature(request, user, "ingest")
    count_sq = (
        select(InboxGhFolderMember.folder_id, func.count().label("cnt"))
        .group_by(InboxGhFolderMember.folder_id)
        .subquery()
    )
    rows = (await session.execute(
        select(InboxGhFolder, func.coalesce(count_sq.c.cnt, 0))
        .outerjoin(count_sq, count_sq.c.folder_id == InboxGhFolder.id)
        .where(InboxGhFolder.user_id == user.id)
        .order_by(InboxGhFolder.name.asc())
    )).all()
    return [InboxGhFolderRead.model_validate({**f.__dict__, "star_count": int(c)}) for f, c in rows]


@router.post("/folders", response_model=InboxGhFolderRead, status_code=status.HTTP_201_CREATED)
async def create_folder(
    request: Request,
    payload: InboxGhFolderCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxGhFolderRead:
    await _require_feature(request, user, "ingest")
    slug = payload.slug or _slugify(payload.name)
    slug = await _unique_slug_folder(session, user.id, slug)
    folder = InboxGhFolder(
        user_id=user.id,
        name=payload.name,
        slug=slug,
        description=payload.description,
        icon=payload.icon,
        parent_id=payload.parent_id,
    )
    session.add(folder)
    await session.commit()
    await session.refresh(folder)
    return InboxGhFolderRead.model_validate({**folder.__dict__, "star_count": 0})


@router.get("/folders/{folder_id}", response_model=InboxGhFolderRead)
async def get_folder(
    request: Request,
    folder_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxGhFolderRead:
    await _require_feature(request, user, "ingest")
    folder = await session.get(InboxGhFolder, folder_id)
    if folder is None or folder.user_id != user.id:
        raise HTTPException(status_code=404, detail="Folder not found")
    cnt = await session.scalar(
        select(func.count(InboxGhFolderMember.gh_star_id))
        .where(InboxGhFolderMember.folder_id == folder_id)
    ) or 0
    return InboxGhFolderRead.model_validate({**folder.__dict__, "star_count": cnt})


@router.put("/folders/{folder_id}", response_model=InboxGhFolderRead)
async def update_folder(
    request: Request,
    folder_id: int,
    payload: InboxGhFolderCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxGhFolderRead:
    await _require_feature(request, user, "ingest")
    folder = await session.get(InboxGhFolder, folder_id)
    if folder is None or folder.user_id != user.id:
        raise HTTPException(status_code=404, detail="Folder not found")
    slug = payload.slug or _slugify(payload.name)
    if slug != folder.slug:
        slug = await _unique_slug_folder(session, user.id, slug, exclude_id=folder_id)
    folder.name = payload.name
    folder.slug = slug
    folder.description = payload.description
    folder.icon = payload.icon
    folder.parent_id = payload.parent_id
    await session.commit()
    await session.refresh(folder)
    cnt = await session.scalar(
        select(func.count(InboxGhFolderMember.gh_star_id))
        .where(InboxGhFolderMember.folder_id == folder_id)
    ) or 0
    return InboxGhFolderRead.model_validate({**folder.__dict__, "star_count": cnt})


@router.delete("/folders/{folder_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_folder(
    request: Request,
    folder_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "ingest")
    folder = await session.get(InboxGhFolder, folder_id)
    if folder is None or folder.user_id != user.id:
        raise HTTPException(status_code=404, detail="Folder not found")
    await session.delete(folder)
    await session.commit()


@router.post("/folders/{folder_id}/stars", status_code=status.HTTP_201_CREATED)
async def add_star_to_folder(
    request: Request,
    folder_id: int,
    star_id: int = Query(),
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> dict:
    await _require_feature(request, user, "ingest")
    folder = await session.get(InboxGhFolder, folder_id)
    if folder is None or folder.user_id != user.id:
        raise HTTPException(status_code=404, detail="Folder not found")
    star = await session.get(InboxGhStar, star_id)
    if star is None or star.user_id != user.id:
        raise HTTPException(status_code=404, detail="Star not found")
    existing = await session.get(InboxGhFolderMember, (folder_id, star_id))
    if existing:
        return {"status": "already_member"}
    session.add(InboxGhFolderMember(folder_id=folder_id, gh_star_id=star_id, added_by="user"))
    await session.commit()
    # Write-back to GitHub List if folder is linked to one
    if folder.gh_list_id and star.gh_node_id:
        try:
            from yoink_inbox.services.gh_write import add_to_gh_list
            await add_to_gh_list(
                request.app.state.session_factory,
                user.id,
                star.gh_node_id,
                folder.gh_list_id,
            )
        except Exception as exc:  # noqa: BLE001
            logger.warning("gh_list write-back failed: %s", exc)
    return {"status": "added"}


@router.delete("/folders/{folder_id}/stars/{star_id}", status_code=status.HTTP_204_NO_CONTENT)
async def remove_star_from_folder(
    request: Request,
    folder_id: int,
    star_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "ingest")
    folder = await session.get(InboxGhFolder, folder_id)
    if folder is None or folder.user_id != user.id:
        raise HTTPException(status_code=404, detail="Folder not found")
    star = await session.get(InboxGhStar, star_id)
    member = await session.get(InboxGhFolderMember, (folder_id, star_id))
    if member:
        await session.delete(member)
        await session.commit()
    # Write-back to GitHub List if folder is linked to one
    if folder.gh_list_id and star and star.gh_node_id:
        try:
            from yoink_inbox.services.gh_write import remove_from_gh_list
            await remove_from_gh_list(
                request.app.state.session_factory,
                user.id,
                star.gh_node_id,
                folder.gh_list_id,
            )
        except Exception:  # noqa: BLE001
            logger.warning("gh_list write-back remove failed")


# ---------------------------------------------------------------------------
# Teams CRUD
# ---------------------------------------------------------------------------


@router.get("/teams", response_model=list[InboxTeamRead])
async def list_teams(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> list[InboxTeamRead]:
    await _require_feature(request, user, "share")
    member_team_ids = (
        await session.scalars(
            select(InboxTeamMember.team_id).where(InboxTeamMember.user_id == user.id)
        )
    ).all()
    teams = (await session.scalars(
        select(InboxTeam)
        .where(
            or_(InboxTeam.owner_user_id == user.id, InboxTeam.id.in_(member_team_ids))
        )
        .order_by(InboxTeam.name.asc())
    )).all()
    result = []
    for team in teams:
        members = (await session.scalars(
            select(InboxTeamMember).where(InboxTeamMember.team_id == team.id)
        )).all()
        d = team.__dict__.copy()
        d["members"] = [InboxTeamMemberRead.model_validate(m) for m in members]
        result.append(InboxTeamRead.model_validate(d))
    return result


@router.post("/teams", response_model=InboxTeamRead, status_code=status.HTTP_201_CREATED)
async def create_team(
    request: Request,
    payload: InboxTeamCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxTeamRead:
    await _require_feature(request, user, "share")
    slug = payload.slug or _slugify(payload.name)
    slug = await _unique_slug_team(session, user.id, slug)
    team = InboxTeam(
        owner_user_id=user.id,
        name=payload.name,
        slug=slug,
        description=payload.description,
    )
    session.add(team)
    await session.flush()
    # owner is also a member
    session.add(InboxTeamMember(team_id=team.id, user_id=user.id, role="owner"))
    await session.commit()
    await session.refresh(team)
    members = (await session.scalars(
        select(InboxTeamMember).where(InboxTeamMember.team_id == team.id)
    )).all()
    d = team.__dict__.copy()
    d["members"] = [InboxTeamMemberRead.model_validate(m) for m in members]
    return InboxTeamRead.model_validate(d)


@router.get("/teams/{team_id}", response_model=InboxTeamRead)
async def get_team(
    request: Request,
    team_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxTeamRead:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None:
        raise HTTPException(status_code=404, detail="Team not found")
    # visible if owner or member
    is_member = await session.scalar(
        select(InboxTeamMember)
        .where(InboxTeamMember.team_id == team_id, InboxTeamMember.user_id == user.id)
    )
    if not is_member and team.owner_user_id != user.id:
        raise HTTPException(status_code=404, detail="Team not found")
    members = (await session.scalars(
        select(InboxTeamMember).where(InboxTeamMember.team_id == team_id)
    )).all()
    d = team.__dict__.copy()
    d["members"] = [InboxTeamMemberRead.model_validate(m) for m in members]
    return InboxTeamRead.model_validate(d)


@router.put("/teams/{team_id}", response_model=InboxTeamRead)
async def update_team(
    request: Request,
    team_id: int,
    payload: InboxTeamCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxTeamRead:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None or team.owner_user_id != user.id:
        raise HTTPException(status_code=403, detail="Not the team owner")
    slug = payload.slug or _slugify(payload.name)
    if slug != team.slug:
        slug = await _unique_slug_team(session, user.id, slug, exclude_id=team_id)
    team.name = payload.name
    team.slug = slug
    team.description = payload.description
    await session.commit()
    await session.refresh(team)
    members = (await session.scalars(
        select(InboxTeamMember).where(InboxTeamMember.team_id == team_id)
    )).all()
    d = team.__dict__.copy()
    d["members"] = [InboxTeamMemberRead.model_validate(m) for m in members]
    return InboxTeamRead.model_validate(d)


@router.delete("/teams/{team_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_team(
    request: Request,
    team_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None or team.owner_user_id != user.id:
        raise HTTPException(status_code=403, detail="Not the team owner")
    await session.delete(team)
    await session.commit()


@router.post("/teams/{team_id}/members", response_model=InboxTeamMemberRead,
             status_code=status.HTTP_201_CREATED)
async def add_team_member(
    request: Request,
    team_id: int,
    payload: InboxTeamMemberUpsert,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxTeamMemberRead:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None:
        raise HTTPException(status_code=404, detail="Team not found")
    caller = await session.scalar(
        select(InboxTeamMember)
        .where(InboxTeamMember.team_id == team_id, InboxTeamMember.user_id == user.id)
    )
    if not caller or caller.role not in ("owner", "admin"):
        raise HTTPException(status_code=403, detail="Need owner or admin role")
    existing = await session.get(InboxTeamMember, (team_id, payload.user_id))
    if existing:
        raise HTTPException(status_code=409, detail="User already a member")
    member = InboxTeamMember(team_id=team_id, user_id=payload.user_id, role=payload.role)
    session.add(member)
    await session.commit()
    await session.refresh(member)
    return InboxTeamMemberRead.model_validate(member)


@router.patch("/teams/{team_id}/members/{uid}", response_model=InboxTeamMemberRead)
async def patch_team_member(
    request: Request,
    team_id: int,
    uid: int,
    payload: InboxTeamMemberUpsert,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxTeamMemberRead:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None:
        raise HTTPException(status_code=404, detail="Team not found")
    if team.owner_user_id != user.id:
        raise HTTPException(status_code=403, detail="Only owner can change roles")
    member = await session.get(InboxTeamMember, (team_id, uid))
    if not member:
        raise HTTPException(status_code=404, detail="Member not found")
    member.role = payload.role
    await session.commit()
    await session.refresh(member)
    return InboxTeamMemberRead.model_validate(member)


@router.delete("/teams/{team_id}/members/{uid}", status_code=status.HTTP_204_NO_CONTENT)
async def remove_team_member(
    request: Request,
    team_id: int,
    uid: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "share")
    team = await session.get(InboxTeam, team_id)
    if team is None:
        raise HTTPException(status_code=404, detail="Team not found")
    # owner can remove anyone; members can only remove themselves
    if user.id != uid and team.owner_user_id != user.id:
        caller = await session.get(InboxTeamMember, (team_id, user.id))
        if not caller or caller.role not in ("owner", "admin"):
            raise HTTPException(status_code=403, detail="Insufficient role")
    member = await session.get(InboxTeamMember, (team_id, uid))
    if member:
        await session.delete(member)
        await session.commit()


# ---------------------------------------------------------------------------
# GitHub stars
# ---------------------------------------------------------------------------


@router.get("/gh_stars/languages", response_model=list[str])
async def list_gh_star_languages(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> list[str]:
    """Distinct non-null languages for the user's starred repos, sorted alpha."""
    await _require_feature(request, user, "gh_sync")
    stmt = (
        select(InboxGhStar.language)
        .where(InboxGhStar.user_id == user.id, InboxGhStar.language.is_not(None))
        .distinct()
        .order_by(InboxGhStar.language.asc())
    )
    rows: list[str] = [r for r in (await session.execute(stmt)).scalars().all() if r is not None]
    return rows


@router.get("/gh_stars", response_model=InboxGhStarListResponse)
async def list_gh_stars(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
    cursor: str | None = Query(default=None),
    limit: int = Query(default=30, ge=1, le=100),
    language: str | None = Query(default=None),
    search: str | None = Query(default=None, min_length=2, max_length=128),
    folder_id: int | None = Query(default=None),
) -> InboxGhStarListResponse:
    """List the user's starred repos with optional filters.

    Sort: starred_at DESC, id DESC (matches GitHub's own \"recent\" view).
    `sync_status` and `last_synced_at` from inbox_gh_sync_state ride along
    so the UI can show \"synced 3h ago\" without a second request.
    """
    await _require_feature(request, user, "gh_sync")

    stmt = select(InboxGhStar).where(InboxGhStar.user_id == user.id)
    if folder_id is not None:
        if folder_id == 0:
            # Unorganised: stars not in any folder (anti-join)
            stmt = stmt.where(
                ~InboxGhStar.id.in_(
                    select(InboxGhFolderMember.gh_star_id)
                )
            )
        else:
            stmt = stmt.join(
                InboxGhFolderMember,
                InboxGhFolderMember.gh_star_id == InboxGhStar.id,
            ).where(InboxGhFolderMember.folder_id == folder_id)
    if language:
        stmt = stmt.where(InboxGhStar.language == language)
    if search:
        pat = f"%{search}%"
        stmt = stmt.where(
            or_(
                InboxGhStar.full_name.ilike(pat),
                InboxGhStar.description.ilike(pat),
            )
        )

    after = _decode_cursor(cursor)
    if after is not None:
        after_dt, after_id = after
        stmt = stmt.where(
            or_(
                InboxGhStar.starred_at < after_dt,
                and_(InboxGhStar.starred_at == after_dt, InboxGhStar.id < after_id),
            )
        )

    stmt = stmt.order_by(
        InboxGhStar.starred_at.desc().nullslast(), InboxGhStar.id.desc()
    ).limit(limit + 1)
    rows = list((await session.execute(stmt)).scalars().all())

    next_cursor: str | None = None
    if len(rows) > limit:
        rows = rows[:limit]
        last = rows[-1]
        if last.starred_at is not None:
            next_cursor = _encode_cursor(last.starred_at, last.id)

    sync_state = await session.get(InboxGhSyncState, user.id)
    return InboxGhStarListResponse(
        items=[InboxGhStarRead.model_validate(r) for r in rows],
        next_cursor=next_cursor,
        sync_status=sync_state.last_status if sync_state else None,
        last_synced_at=sync_state.last_synced_at if sync_state else None,
    )


# ---------------------------------------------------------------------------
# Rules
# ---------------------------------------------------------------------------

_VALID_TRIGGERS = frozenset({"item_ingested", "item_classified", "star_synced"})


@router.get("/rules", response_model=list[InboxRuleRead])
async def list_rules(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> list[InboxRuleRead]:
    """List the user's rules ordered by priority, then id."""
    await _require_feature(request, user, "ingest")
    rows = (
        await session.scalars(
            select(InboxRule)
            .where(InboxRule.user_id == user.id)
            .order_by(InboxRule.priority.asc(), InboxRule.id.asc())
        )
    ).all()
    return [InboxRuleRead.model_validate(r) for r in rows]


@router.post("/rules", response_model=InboxRuleRead, status_code=status.HTTP_201_CREATED)
async def create_rule(
    request: Request,
    payload: InboxRuleCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxRuleRead:
    """Create a new automation rule."""
    await _require_feature(request, user, "ingest")
    if payload.trigger not in _VALID_TRIGGERS:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=f"trigger must be one of {sorted(_VALID_TRIGGERS)}",
        )
    rule = InboxRule(
        user_id=user.id,
        name=payload.name,
        enabled=payload.enabled,
        priority=payload.priority,
        trigger=payload.trigger,
        conditions=payload.conditions,
        actions=payload.actions,
    )
    session.add(rule)
    await session.commit()
    await session.refresh(rule)
    return InboxRuleRead.model_validate(rule)


@router.get("/rules/{rule_id}", response_model=InboxRuleRead)
async def get_rule(
    request: Request,
    rule_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxRuleRead:
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")
    return InboxRuleRead.model_validate(rule)


@router.patch("/rules/{rule_id}", response_model=InboxRuleRead)
async def update_rule(
    request: Request,
    rule_id: int,
    payload: InboxRuleUpdate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxRuleRead:
    """Partial-update a rule; omit any field to leave it unchanged."""
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")
    if payload.trigger is not None and payload.trigger not in _VALID_TRIGGERS:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=f"trigger must be one of {sorted(_VALID_TRIGGERS)}",
        )
    for field, val in payload.model_dump(exclude_unset=True).items():
        setattr(rule, field, val)
    await session.commit()
    await session.refresh(rule)
    return InboxRuleRead.model_validate(rule)


@router.put("/rules/{rule_id}", response_model=InboxRuleRead)
async def replace_rule(
    request: Request,
    rule_id: int,
    payload: InboxRuleCreate,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxRuleRead:
    """Full replace of a rule."""
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")
    if payload.trigger not in _VALID_TRIGGERS:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=f"trigger must be one of {sorted(_VALID_TRIGGERS)}",
        )
    rule.name = payload.name
    rule.enabled = payload.enabled
    rule.priority = payload.priority
    rule.trigger = payload.trigger
    rule.conditions = payload.conditions
    rule.actions = payload.actions
    await session.commit()
    await session.refresh(rule)
    return InboxRuleRead.model_validate(rule)


@router.delete("/rules/{rule_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_rule(
    request: Request,
    rule_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")
    await session.delete(rule)
    await session.commit()


@router.post("/rules/{rule_id}/test", response_model=InboxRuleTestResult)
async def test_rule(
    request: Request,
    rule_id: int,
    item_id: int | None = Query(default=None),
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> InboxRuleTestResult:
    """Dry-run a rule against a specific item (no actions executed)."""
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")
    if item_id is None:
        raise HTTPException(status_code=400, detail="item_id query param required")
    item = await session.get(InboxItem, item_id)
    if item is None or item.user_id != user.id:
        raise HTTPException(status_code=404, detail="Item not found")

    from yoink_inbox.services.rules import evaluate_rule_conditions

    cat_rows = await session.scalars(
        select(InboxCategory.name)
        .join(InboxItemCategory, InboxItemCategory.category_id == InboxCategory.id)
        .where(InboxItemCategory.item_id == item_id)
    )
    category_names = list(cat_rows.all())
    conditions_result = evaluate_rule_conditions(rule, item, category_names)
    matched = all(r["passed"] for r in conditions_result) if conditions_result else True
    return InboxRuleTestResult(
        rule_id=rule_id,
        matched=matched,
        conditions_result=conditions_result,
        actions_would_fire=list(rule.actions or []) if matched else [],
    )


@router.post("/rules/{rule_id}/run", status_code=status.HTTP_202_ACCEPTED)
async def run_rule_sweep(
    request: Request,
    rule_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> dict:
    """Apply one rule against all non-archived items of the user (max 500)."""
    await _require_feature(request, user, "ingest")
    rule = await session.get(InboxRule, rule_id)
    if rule is None or rule.user_id != user.id:
        raise HTTPException(status_code=404, detail="Rule not found")

    from yoink_inbox.services.rules import run_rules

    items = (await session.scalars(
        select(InboxItem)
        .where(InboxItem.user_id == user.id, InboxItem.archived_at.is_(None))
        .order_by(InboxItem.created_at.desc())
        .limit(500)
    )).all()

    fired = 0
    for item in items:
        fired += await run_rules(session, user_id=user.id, trigger=rule.trigger, item_id=item.id)
    await session.commit()
    return {"fired": fired, "scanned": len(items)}


@router.put("/gh_stars/{star_id}/star", status_code=status.HTTP_204_NO_CONTENT)
async def star_repo(
    request: Request,
    star_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    """Star the repo on GitHub using the user's public_repo token."""
    await _require_feature(request, user, "gh_write")
    star = await session.get(InboxGhStar, star_id)
    if star is None or star.user_id != user.id:
        raise HTTPException(status_code=404, detail="Star not found")
    owner, repo = star.full_name.split("/", 1)
    sf = request.app.state.session_factory
    from yoink_inbox.services.gh_write import star_repo as _star
    try:
        await _star(sf, user.id, owner, repo)
    except PermissionError as exc:
        raise HTTPException(status_code=403, detail=str(exc))
    star.can_unstar = True
    await session.commit()


@router.delete("/gh_stars/{star_id}/star", status_code=status.HTTP_204_NO_CONTENT)
async def unstar_repo(
    request: Request,
    star_id: int,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
) -> None:
    """Unstar the repo on GitHub using the user's public_repo token."""
    await _require_feature(request, user, "gh_write")
    star = await session.get(InboxGhStar, star_id)
    if star is None or star.user_id != user.id:
        raise HTTPException(status_code=404, detail="Star not found")
    owner, repo = star.full_name.split("/", 1)
    sf = request.app.state.session_factory
    from yoink_inbox.services.gh_write import unstar_repo as _unstar
    try:
        await _unstar(sf, user.id, owner, repo)
    except PermissionError as exc:
        raise HTTPException(status_code=403, detail=str(exc))
    # Soft-remove: mark can_unstar=False but keep the local star row
    # (GitHub will stop returning it on the next sync; hard-delete happens then).
    star.can_unstar = False
    await session.commit()


@router.post("/gh_stars/sync", status_code=status.HTTP_202_ACCEPTED)
async def trigger_gh_sync(
    request: Request,
    user: User = Depends(get_current_user),
) -> dict[str, str]:
    """Enqueue a fresh /user/starred sync; inline fallback if Redis is down."""
    await _require_feature(request, user, "gh_sync")
    bot_data = getattr(request.app.state, "bot_data", {}) or {}
    arq = bot_data.get("inbox_arq_pool")
    if arq is not None:
        await arq.enqueue_job(
            "sync_user_stars", user.id, _queue_name="inbox:default"
        )
        return {"status": "queued"}

    sf = request.app.state.session_factory
    from yoink_inbox.services.gh_stars import run_sync

    res = await run_sync(sf, user.id)
    return {"status": res.status}
