"""Async engine and session factory."""
from __future__ import annotations

from collections.abc import AsyncGenerator

from sqlalchemy.ext.asyncio import (
    AsyncSession,
    async_sessionmaker,
    create_async_engine,
)

from yoink.core.db.base import Base

_engine = None
_session_factory: async_sessionmaker | None = None


def init_engine(url: str, echo: bool = False) -> None:
    global _engine, _session_factory
    _engine = create_async_engine(url, echo=echo, pool_pre_ping=True)
    _session_factory = async_sessionmaker(_engine, expire_on_commit=False)


async def create_tables() -> None:
    """Create all tables registered in Base.metadata.

    All plugin models inherit from Base (single DeclarativeBase), so importing
    them before this call is sufficient - no extra metadata arguments needed.
    """
    async with _engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)


async def get_session() -> AsyncGenerator[AsyncSession, None]:
    async with _session_factory() as session:
        yield session


def get_session_factory() -> async_sessionmaker:
    if _session_factory is None:
        raise RuntimeError("Engine not initialized. Call init_engine() first.")
    return _session_factory
