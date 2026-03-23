"""FastAPI dependencies for M2M (API key) authentication."""
from __future__ import annotations

import logging
from collections.abc import AsyncGenerator, Callable
from datetime import datetime, timezone

from fastapi import Depends, HTTPException, Request, Security, status
from fastapi.security import APIKeyHeader
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession

from yoink.core.auth.apikey import has_scope, hash_key, is_expired
from yoink.core.db.models import ApiKey

logger = logging.getLogger(__name__)

api_key_header = APIKeyHeader(name="X-Api-Key", auto_error=False)


async def _get_db(request: Request) -> AsyncGenerator[AsyncSession, None]:
    session_factory = request.app.state.session_factory
    async with session_factory() as session:
        yield session


async def get_api_key(
    raw_key: str | None = Security(api_key_header),
    session: AsyncSession = Depends(_get_db),
) -> ApiKey:
    """Validate X-Api-Key header and return the ApiKey row."""
    if not raw_key:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Missing X-Api-Key header",
        )
    key_hash_val = hash_key(raw_key)
    result = await session.execute(
        select(ApiKey).where(ApiKey.key_hash == key_hash_val)
    )
    key_row = result.scalar_one_or_none()

    if key_row is None:
        logger.warning("Invalid API key attempt (prefix: %s...)", raw_key[:8])
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid API key",
        )

    if key_row.revoked:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="API key has been revoked",
        )

    if is_expired(key_row.expires_at):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="API key has expired",
        )

    await session.execute(
        update(ApiKey)
        .where(ApiKey.id == key_row.id)
        .values(last_used_at=datetime.now(timezone.utc))
    )
    await session.commit()

    return key_row


def require_scope(scope: str) -> Callable:
    """FastAPI dependency: require a specific scope on the API key."""
    async def _check(key: ApiKey = Depends(get_api_key)) -> ApiKey:
        if not has_scope(key.scopes, scope):
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"API key missing required scope: {scope}",
            )
        return key
    return _check
