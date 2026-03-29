"""User photo proxy and backfill endpoints."""
from __future__ import annotations

import httpx
from fastapi import APIRouter, Depends, Request
from fastapi.responses import Response
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_db
from yoink.core.api.exceptions import NotFoundError
from yoink.core.api.photo import bot_api_params, resolve_chat_photo
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import User, UserRole

router = APIRouter(prefix="/users", tags=["users"])


@router.get("/{user_id}/photo", summary="Proxy user profile photo")
async def get_user_photo(user_id: int, request: Request) -> Response:
    """Stream user's current Telegram profile photo (fetched live via Bot API)."""
    bot_api_url, bot_token = bot_api_params(request.app.state)
    data = await resolve_chat_photo(bot_api_url, bot_token, user_id)
    if not data:
        raise NotFoundError("No photo available")
    return Response(
        content=data,
        media_type="image/jpeg",
        headers={"Cache-Control": "public, max-age=86400"},
    )


@router.post("/photos/sync", summary="Backfill user photos via getChat (owner only)")
async def sync_user_photos(
    request: Request,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict:
    """Fetch profile photos for all users missing them via Bot API getChat."""
    bot_api_url = request.app.state.settings.bot_api_url if hasattr(request.app.state.settings, "bot_api_url") else "https://api.telegram.org"
    bot_token = request.app.state.settings.bot_token

    users_without_photo = (await session.execute(
        select(User).where(User.photo_url.is_(None))
    )).scalars().all()

    updated = 0
    for u in users_without_photo:
        try:
            async with httpx.AsyncClient(timeout=5) as client:
                r = await client.get(
                    f"{bot_api_url}/bot{bot_token}/getChat",
                    params={"chat_id": u.id},
                )
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
