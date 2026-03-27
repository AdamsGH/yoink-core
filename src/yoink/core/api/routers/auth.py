"""POST /auth/token and /auth/dev."""
from __future__ import annotations

import json
import logging
from datetime import datetime, timezone

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_db
from yoink.core.api.schemas import TelegramInitDataRequest, TokenResponse
from yoink.core.auth.jwt import create_access_token
from yoink.core.auth.telegram import verify_init_data
from yoink.core.db.models import User, UserRole

logger = logging.getLogger(__name__)

router = APIRouter(prefix="/auth", tags=["auth"], responses={401: {"description": "Invalid or expired token"}})


def _parse_tg_user(params: dict) -> tuple[int, str | None, str | None]:
    """Extract (id, username, first_name) from verified initData params."""
    user_json = params.get("user")
    if user_json:
        try:
            user_obj = json.loads(user_json)
            return (
                int(user_obj["id"]),
                user_obj.get("username"),
                user_obj.get("first_name"),
            )
        except (KeyError, ValueError, TypeError) as exc:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="initData user field is malformed",
            ) from exc
    raise HTTPException(
        status_code=status.HTTP_400_BAD_REQUEST,
        detail="initData missing user field",
    )


@router.post(
    "/token",
    response_model=TokenResponse,
    summary="Authenticate via Telegram WebApp",
    description=(
        "Verify Telegram WebApp `initData` HMAC signature and return a JWT.\n\n"
        "New users are auto-created. The owner (configured `OWNER_ID`) always gets the `owner` role. "
        "If `bot_access_mode=approved_only`, new users start as `restricted`."
    ),
)
async def auth_token(
    body: TelegramInitDataRequest,
    request: Request,
    session: AsyncSession = Depends(get_db),
) -> TokenResponse:
    settings = request.app.state.settings

    try:
        params = verify_init_data(body.init_data, settings.bot_token)
    except ValueError as exc:
        logger.warning("initData verification failed: %s", exc)
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid Telegram initData",
        ) from exc

    user_id, username, first_name = _parse_tg_user(params)

    user = await session.get(User, user_id)
    if user is None:
        from yoink.core.db.models import BotSetting
        row = await session.get(BotSetting, "bot_access_mode")
        mode = row.value if row else "open"
        role = UserRole.restricted if mode == "approved_only" else UserRole.user
        if user_id == settings.owner_id:
            role = UserRole.owner
        user = User(id=user_id, username=username, first_name=first_name, role=role)
        session.add(user)
    else:
        if username is not None:
            user.username = username
        if first_name is not None:
            user.first_name = first_name
        if user_id == settings.owner_id and user.role != UserRole.owner:
            user.role = UserRole.owner
        user.updated_at = datetime.now(timezone.utc)

    await session.commit()
    await session.refresh(user)

    token = create_access_token(
        user_id=user.id,
        role=user.role.value,
        secret=settings.api_secret_key,
        expires_minutes=settings.api_token_expire_minutes,
        first_name=user.first_name,
        username=user.username,
    )
    return TokenResponse(access_token=token, user_id=user.id, role=user.role.value)


@router.post(
    "/dev",
    response_model=TokenResponse,
    summary="Dev token (disabled in production)",
    description="Generate a token for any `user_id` without Telegram verification. Only available when `DEV_AUTH_ENABLED=true`.",
    include_in_schema=True,
)
async def auth_dev(
    request: Request,
    user_id: int = Query(...),
    role: UserRole = Query(UserRole.user),
) -> TokenResponse:
    settings = request.app.state.settings
    if not getattr(settings, "dev_auth_enabled", False):
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND)
    token = create_access_token(
        user_id=user_id,
        role=role.value,
        secret=settings.api_secret_key,
        expires_minutes=settings.api_token_expire_minutes,
    )
    return TokenResponse(access_token=token, user_id=user_id, role=role.value)
