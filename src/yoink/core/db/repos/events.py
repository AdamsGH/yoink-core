"""Event log repository - append only."""
from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.db.models import Event


class EventRepo:
    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._sf = session_factory

    async def log(
        self,
        plugin: str,
        event_type: str,
        user_id: int | None = None,
        group_id: int | None = None,
        thread_id: int | None = None,
        payload: dict[str, Any] | None = None,
    ) -> None:
        async with self._sf() as s:
            s.add(Event(
                plugin=plugin,
                event_type=event_type,
                user_id=user_id,
                group_id=group_id,
                thread_id=thread_id,
                payload=payload,
            ))
            await s.commit()
