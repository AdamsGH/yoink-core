"""Forum topic proxy endpoints via user-mode session."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status

from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole
from yoink.core.services.user_session import UserSessionError, UserSessionService

router = APIRouter(prefix="/forum", tags=["forum"])


def _svc(request: Request) -> UserSessionService:
    svc: UserSessionService | None = None
    if hasattr(request.app.state, "bot_data"):
        svc = request.app.state.bot_data.get("user_session")
    if svc is None or not svc.is_available():
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="User-mode session not available. Run: just tg-login +<phone>",
        )
    return svc


def _wrap(exc: UserSessionError) -> HTTPException:
    code = status.HTTP_502_BAD_GATEWAY
    if "unauthorized" in str(exc).lower():
        code = status.HTTP_401_UNAUTHORIZED
    return HTTPException(status_code=code, detail=str(exc))


@router.get("/topics/{chat_id}")
async def get_forum_topics(
    chat_id: int,
    request: Request,
    query: str = Query("", description="Search filter"),
    limit: int = Query(100, ge=1, le=100, description="Maximum number of records to return"),
    offset_date: int = Query(0, description="Telegram pagination: date offset (Unix timestamp)"),
    offset_message_id: int = Query(0, description="Telegram pagination: message ID offset"),
    offset_forum_topic_id: int = Query(0, description="Telegram pagination: forum topic ID offset"),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """
    List forum topics for a supergroup chat.
    Returns total_count, topics[], and next_offset_* for pagination.
    Each topic: message_thread_id, name, icon_color, is_closed, is_hidden,
    is_general, is_pinned, unread_count, creation_date.
    """
    svc = _svc(request)
    try:
        return await svc.get_forum_topics(
            chat_id=chat_id,
            query=query,
            limit=limit,
            offset_date=offset_date,
            offset_message_id=offset_message_id,
            offset_forum_topic_id=offset_forum_topic_id,
        )
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/topics/{chat_id}/{thread_id}")
async def get_forum_topic(
    chat_id: int,
    thread_id: int,
    request: Request,
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """Get a single forum topic by message_thread_id."""
    svc = _svc(request)
    try:
        return await svc.get_forum_topic(chat_id=chat_id, message_thread_id=thread_id)
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/topics/{chat_id}/{thread_id}/link")
async def get_forum_topic_link(
    chat_id: int,
    thread_id: int,
    request: Request,
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """Get public t.me link for a forum topic. Returns {link, is_public}."""
    svc = _svc(request)
    try:
        return await svc.get_forum_topic_link(chat_id=chat_id, message_thread_id=thread_id)
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/search/{chat_id}")
async def search_chat_messages(
    chat_id: int,
    request: Request,
    query: str = Query("", description="Text to search for"),
    thread_id: int = Query(0, description="Restrict search to a forum topic"),
    from_message_id: int = Query(0, description="Start from this message ID (pagination)"),
    offset: int = Query(0, ge=0, description="Pagination offset (number of records to skip)"),
    limit: int = Query(50, ge=1, le=100, description="Maximum number of records to return"),
    filter: str = Query(
        "",
        description=(
            "Media type filter: animation, audio, document, photo, video, "
            "voice_note, photo_and_video, url, mention"
        ),
    ),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """
    Search messages in a chat. Returns total_count, next_from_message_id, messages[].
    Supports per-topic search via thread_id, media type filters, and pagination.
    """
    svc = _svc(request)
    try:
        return await svc.search_chat_messages(
            chat_id=chat_id,
            query=query,
            message_thread_id=thread_id,
            from_message_id=from_message_id,
            offset=offset,
            limit=limit,
            filter=filter,
        )
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/history/{chat_id}")
async def get_chat_history(
    chat_id: int,
    request: Request,
    from_message_id: int = Query(0, description="Start from this message ID (0 = latest)"),
    offset: int = Query(0, ge=0, description="Pagination offset (number of records to skip)"),
    limit: int = Query(50, ge=1, le=100, description="Maximum number of records to return"),
    only_local: bool = Query(False, description="Return only locally cached messages"),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """
    Fetch chat history in reverse chronological order.
    Returns messages[] array. Use from_message_id + offset for pagination.
    """
    svc = _svc(request)
    try:
        return await svc.get_chat_history(
            chat_id=chat_id,
            from_message_id=from_message_id,
            offset=offset,
            limit=limit,
            only_local=only_local,
        )
    except UserSessionError as exc:
        raise _wrap(exc) from exc
