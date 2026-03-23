"""BotSetting key-value repository."""
from __future__ import annotations

from typing import Any

from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.db.models import BotSetting, UserRole

DEFAULTS: dict[str, Any] = {
    "browser_cookies_min_role": UserRole.owner.value,
    "bot_access_mode": "open",
    "inline_storage_chat_id": None,
    "inline_storage_thread_id": None,
}


class BotSettingsRepo:
    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._sf = session_factory

    async def get(self, key: str) -> str | None:
        async with self._sf() as s:
            row = await s.get(BotSetting, key)
            return row.value if row else None

    async def set(self, key: str, value: str) -> None:
        async with self._sf() as s:
            row = await s.get(BotSetting, key)
            if row is None:
                s.add(BotSetting(key=key, value=value))
            else:
                row.value = value
            await s.commit()

    async def get_all(self) -> dict[str, str | None]:
        async with self._sf() as s:
            rows = (await s.execute(select(BotSetting))).scalars().all()
            result = dict(DEFAULTS)
            for row in rows:
                result[row.key] = row.value
            return result

    async def get_browser_cookies_min_role(self) -> UserRole:
        val = await self.get("browser_cookies_min_role")
        if val is None:
            return UserRole.owner
        try:
            return UserRole(val)
        except ValueError:
            return UserRole.owner

    async def get_bot_access_mode(self) -> str:
        val = await self.get("bot_access_mode")
        return val if val in ("open", "approved_only") else "open"

    async def get_inline_storage(self) -> tuple[int | None, int | None]:
        """Return (chat_id, thread_id) for global inline storage, or (None, None)."""
        chat_raw = await self.get("inline_storage_chat_id")
        thread_raw = await self.get("inline_storage_thread_id")
        try:
            chat_id = int(chat_raw) if chat_raw else None
        except ValueError:
            chat_id = None
        try:
            thread_id = int(thread_raw) if thread_raw else None
        except ValueError:
            thread_id = None
        return chat_id, thread_id
