"""Bot-wide admin settings."""
from __future__ import annotations

from typing import Any

from fastapi import APIRouter, Depends
from pydantic import BaseModel
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_db
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import BotSetting, User, UserRole
from yoink.core.db.repos.bot_settings import DEFAULTS

router = APIRouter(prefix="/bot-settings", tags=["bot-settings"])


class BotSettingsUpdateRequest(BaseModel):
    bot_access_mode: str | None = None
    browser_cookies_min_role: str | None = None
    inline_storage_chat_id: int | None = None
    inline_storage_thread_id: int | None = None


@router.get("")
async def get_bot_settings(
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> dict[str, Any]:
    from sqlalchemy import select
    rows = (await session.execute(select(BotSetting))).scalars().all()
    result: dict[str, Any] = dict(DEFAULTS)
    for row in rows:
        result[row.key] = row.value
    return result


@router.patch("")
async def update_bot_settings(
    body: BotSettingsUpdateRequest,
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> dict[str, Any]:
    updates: dict[str, Any] = body.model_dump(exclude_none=True)
    for key, value in updates.items():
        row = await session.get(BotSetting, key)
        if row is None:
            session.add(BotSetting(key=key, value=str(value)))
        else:
            row.value = str(value)
    await session.commit()
    return {"ok": True}
