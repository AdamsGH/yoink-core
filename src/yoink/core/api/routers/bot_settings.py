"""Bot-wide admin settings."""
from __future__ import annotations

from typing import Any

from fastapi import APIRouter, Depends
from pydantic import BaseModel
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.api.deps import get_db
from yoink.core.auth.rbac import require_role
from yoink.core.db.models import BotSetting, User, UserRole
from yoink.core.db.repos.bot_settings import DEFAULTS, BotSettingsRepo
from yoink.core.plugin import get_all_features

router = APIRouter(prefix="/bot-settings", tags=["bot-settings"])


class BotSettingsUpdateRequest(BaseModel):
    bot_access_mode: str | None = None
    browser_cookies_min_role: str | None = None
    inline_storage_chat_id: int | None = None
    inline_storage_thread_id: int | None = None


class TagMapEntry(BaseModel):
    tag: str
    features: list[str]


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


@router.get("/tag-map", response_model=list[TagMapEntry])
async def get_tag_map(
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[TagMapEntry]:
    """Return tag -> features mapping as a list of entries."""
    from sqlalchemy.ext.asyncio import async_sessionmaker
    sf: async_sessionmaker = session.get_bind()  # type: ignore[arg-type]
    repo = BotSettingsRepo.__new__(BotSettingsRepo)
    row = await session.get(BotSetting, "tag_map")
    import json
    raw = row.value if row else "{}"
    try:
        data: dict[str, list[str]] = json.loads(raw) if raw else {}
    except (json.JSONDecodeError, TypeError):
        data = {}
    return [TagMapEntry(tag=tag, features=feats) for tag, feats in sorted(data.items())]


@router.put("/tag-map", response_model=list[TagMapEntry])
async def set_tag_map(
    entries: list[TagMapEntry],
    session: AsyncSession = Depends(get_db),
    _: User = Depends(require_role(UserRole.owner)),
) -> list[TagMapEntry]:
    """Replace the entire tag -> features mapping."""
    import json
    mapping = {e.tag: e.features for e in entries if e.tag.strip()}
    value = json.dumps(mapping, ensure_ascii=False)
    row = await session.get(BotSetting, "tag_map")
    if row is None:
        session.add(BotSetting(key="tag_map", value=value))
    else:
        row.value = value
    await session.commit()
    return [TagMapEntry(tag=tag, features=feats) for tag, feats in sorted(mapping.items())]


@router.get("/available-features", response_model=list[dict])
async def get_available_features(
    _: User = Depends(require_role(UserRole.admin, UserRole.owner)),
) -> list[dict]:
    """Return all declared plugin features for use in tag-map dropdowns."""
    return [
        {
            "key": f"{f.plugin}:{f.feature}",
            "plugin": f.plugin,
            "feature": f.feature,
            "label": f.label,
            "description": f.description,
        }
        for f in get_all_features()
    ]
