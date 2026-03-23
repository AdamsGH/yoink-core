"""Message-level proxy endpoints via user-mode session."""
from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status

from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole
from yoink.core.services.user_session import UserSessionError, UserSessionService

router = APIRouter(prefix="/messages", tags=["messages"])


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


@router.get("/{chat_id}/{message_id}/viewers")
async def get_message_viewers(
    chat_id: int,
    message_id: int,
    request: Request,
    _: User = Depends(require_role(UserRole.owner)),
) -> list[dict]:
    """
    Get users who viewed a message (only works for messages in supergroups
    that are not older than 7 days). Returns [{user_id, view_date}].
    """
    svc = _svc(request)
    try:
        return await svc.call("getMessageViewers", chat_id=chat_id, message_id=message_id)
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/{chat_id}/{message_id}/link")
async def get_message_link(
    chat_id: int,
    message_id: int,
    request: Request,
    media_timestamp: int = Query(0, description="Timestamp in seconds for media messages"),
    for_album: bool = Query(False, description="Return link for the whole album"),
    in_message_thread: bool = Query(False, description="Link points to the message in its thread"),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """Get a shareable link to a message. Returns {link, is_public}."""
    svc = _svc(request)
    try:
        return await svc.call(
            "getMessageLink",
            chat_id=chat_id,
            message_id=message_id,
            media_timestamp=media_timestamp,
            for_album="true" if for_album else "false",
            in_message_thread="true" if in_message_thread else "false",
        )
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/{chat_id}/{message_id}/read-date")
async def get_message_read_date(
    chat_id: int,
    message_id: int,
    request: Request,
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """
    Get when a message was read by the recipient (private chats only).
    Returns {status: read|unread|too_old|user_privacy_restricted|my_privacy_restricted,
             read_date?}.
    """
    svc = _svc(request)
    try:
        return await svc.call("getMessageReadDate", chat_id=chat_id, message_id=message_id)
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/{chat_id}/{message_id}/thread")
async def get_message_thread(
    chat_id: int,
    message_id: int,
    request: Request,
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """
    Get the thread a message belongs to.
    Returns {chat_id, message_thread_id, unread_message_count, messages[]}.
    """
    svc = _svc(request)
    try:
        return await svc.call("getMessageThread", chat_id=chat_id, message_id=message_id)
    except UserSessionError as exc:
        raise _wrap(exc) from exc


@router.get("/{chat_id}/by-date")
async def get_chat_message_by_date(
    chat_id: int,
    request: Request,
    date: int = Query(..., description="Unix timestamp - returns the nearest message"),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """Find the message closest to a given Unix timestamp in a chat."""
    svc = _svc(request)
    try:
        return await svc.call("getChatMessageByDate", chat_id=chat_id, date=date)
    except UserSessionError as exc:
        raise _wrap(exc) from exc
