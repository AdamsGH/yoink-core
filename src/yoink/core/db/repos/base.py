"""Generic async repository base."""
from __future__ import annotations

from typing import Any, Generic, TypeVar

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from yoink.core.db.base import Base

T = TypeVar("T", bound=Base)


class BaseRepo(Generic[T]):
    model: type[T]

    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._sf = session_factory

    async def get(self, id: Any) -> T | None:
        async with self._sf() as s:
            return await s.get(self.model, id)

    async def create(self, **kwargs: Any) -> T:
        async with self._sf() as s:
            obj = self.model(**kwargs)
            s.add(obj)
            await s.commit()
            await s.refresh(obj)
            return obj

    async def update(self, id: Any, **kwargs: Any) -> T | None:
        async with self._sf() as s:
            obj = await s.get(self.model, id)
            if obj is None:
                return None
            for k, v in kwargs.items():
                setattr(obj, k, v)
            await s.commit()
            await s.refresh(obj)
            return obj

    async def delete(self, id: Any) -> bool:
        async with self._sf() as s:
            obj = await s.get(self.model, id)
            if obj is None:
                return False
            await s.delete(obj)
            await s.commit()
            return True

    async def list_all(self, **filters: Any) -> list[T]:
        async with self._sf() as s:
            stmt = select(self.model)
            for k, v in filters.items():
                stmt = stmt.where(getattr(self.model, k) == v)
            result = await s.execute(stmt)
            return list(result.scalars().all())
