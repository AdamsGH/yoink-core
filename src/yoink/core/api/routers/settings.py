"""User settings (core fields: language, theme)."""
from __future__ import annotations

from fastapi import APIRouter, Depends
from pydantic import BaseModel
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_current_user, get_db
from yoink.core.db.models import User

router = APIRouter(prefix="/settings", tags=["settings"])


class SettingsResponse(BaseModel):
    user_id: int
    language: str
    theme: str


class SettingsUpdateRequest(BaseModel):
    language: str | None = None
    theme: str | None = None


@router.get("", response_model=SettingsResponse, summary="Get my settings")
async def get_settings(
    current_user: User = Depends(get_current_user),
) -> SettingsResponse:
    return SettingsResponse(
        user_id=current_user.id,
        language=current_user.language,
        theme=current_user.theme,
    )


@router.patch("", response_model=SettingsResponse, summary="Update my settings", description="Update `language` (`en`/`ru`) and/or `theme` (`dark`/`light`/`system`).")
async def update_settings(
    body: SettingsUpdateRequest,
    session: AsyncSession = Depends(get_db),
    current_user: User = Depends(get_current_user),
) -> SettingsResponse:
    if body.language is not None:
        current_user.language = body.language
    if body.theme is not None:
        current_user.theme = body.theme
    await session.commit()
    await session.refresh(current_user)
    return SettingsResponse(
        user_id=current_user.id,
        language=current_user.language,
        theme=current_user.theme,
    )
