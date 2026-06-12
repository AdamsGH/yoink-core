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
from datetime import datetime
from typing import TYPE_CHECKING

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy import and_, delete as sql_delete, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.auth.effective_features import EffectiveFeatureResolver
from yoink.core.db.models import User, UserRole
from yoink_inbox.api.schemas import (
    InboxCategoryRead,
    InboxGhStarListResponse,
    InboxGhStarRead,
    InboxItemCategoryRef,
    InboxItemCreate,
    InboxItemListResponse,
    InboxItemRead,
)
from yoink_inbox.storage.models import (
    InboxCategory,
    InboxGhStar,
    InboxGhSyncState,
    InboxItem,
    InboxItemCategory,
    InboxTeamMember,
)

if TYPE_CHECKING:
    pass

logger = logging.getLogger(__name__)

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
# GitHub stars
# ---------------------------------------------------------------------------


@router.get("/gh_stars", response_model=InboxGhStarListResponse)
async def list_gh_stars(
    request: Request,
    user: User = Depends(get_current_user),
    session: AsyncSession = Depends(get_db),
    cursor: str | None = Query(default=None),
    limit: int = Query(default=30, ge=1, le=100),
    language: str | None = Query(default=None),
    search: str | None = Query(default=None, min_length=2, max_length=128),
) -> InboxGhStarListResponse:
    """List the user's starred repos with optional filters.

    Sort: starred_at DESC, id DESC (matches GitHub's own \"recent\" view).
    `sync_status` and `last_synced_at` from inbox_gh_sync_state ride along
    so the UI can show \"synced 3h ago\" without a second request.
    """
    await _require_feature(request, user, "gh_sync")

    stmt = select(InboxGhStar).where(InboxGhStar.user_id == user.id)
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
