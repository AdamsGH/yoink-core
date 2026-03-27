"""User repository."""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.db.models import BotSetting, User, UserRole
from yoink.core.db.repos.base import BaseRepo


class UserRepo(BaseRepo[User]):
    model = User

    def __init__(self, session_factory: async_sessionmaker, owner_id: int) -> None:
        super().__init__(session_factory)
        self._owner_id = owner_id

    async def _default_role(self, session) -> UserRole:
        row = await session.get(BotSetting, "bot_access_mode")
        mode = row.value if row else "open"
        return UserRole.restricted if mode == "approved_only" else UserRole.user

    async def get_or_create(
        self,
        user_id: int,
        username: str | None = None,
        first_name: str | None = None,
        is_premium: bool = False,
        photo_url: str | None = None,
        **_: Any,
    ) -> User:
        async with self._sf() as session:
            user = await session.get(User, user_id)
            if user is None:
                role = UserRole.owner if user_id == self._owner_id else await self._default_role(session)
                user = User(
                    id=user_id,
                    username=username,
                    first_name=first_name,
                    is_premium=is_premium,
                    photo_url=photo_url,
                    role=role,
                )
                session.add(user)
                await session.commit()
                await session.refresh(user)
            else:
                changed = False
                if username is not None and user.username != username:
                    user.username = username
                    changed = True
                if first_name is not None and user.first_name != first_name:
                    user.first_name = first_name
                    changed = True
                if is_premium != user.is_premium:
                    user.is_premium = is_premium
                    changed = True
                if photo_url is not None and user.photo_url != photo_url:
                    user.photo_url = photo_url
                    changed = True
                if changed:
                    user.updated_at = datetime.now(timezone.utc)
                    await session.commit()
                    await session.refresh(user)
            return user

    async def update(self, user_id: int, **kwargs: Any) -> User | None:
        async with self._sf() as session:
            user = await session.get(User, user_id)
            if user is None:
                return None
            for k, v in kwargs.items():
                setattr(user, k, v)
            user.updated_at = datetime.now(timezone.utc)
            await session.commit()
            await session.refresh(user)
            return user

    async def list_paginated(self, offset: int = 0, limit: int = 50) -> tuple[list[User], int]:
        from sqlalchemy import func
        async with self._sf() as session:
            total_result = await session.execute(select(func.count(User.id)))
            total = total_result.scalar_one()
            result = await session.execute(
                select(User).order_by(User.created_at.desc()).offset(offset).limit(limit)
            )
            return list(result.scalars().all()), total
