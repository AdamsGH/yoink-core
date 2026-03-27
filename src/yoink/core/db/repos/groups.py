"""Group, ThreadPolicy, UserGroupPolicy repositories."""
from __future__ import annotations

from collections.abc import Sequence

from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import async_sessionmaker

from yoink.core.db.models import Group, ThreadPolicy, UserGroupPolicy, UserRole
from yoink.core.db.repos.base import BaseRepo


class GroupRepo(BaseRepo[Group]):
    model = Group

    def __init__(self, session_factory: async_sessionmaker) -> None:
        super().__init__(session_factory)

    async def upsert(
        self,
        group_id: int,
        title: str | None = None,
        enabled: bool = True,
        auto_grant_role: UserRole = UserRole.user,
        allow_pm: bool = True,
    ) -> Group:
        async with self._sf() as s:
            group = await s.get(Group, group_id)
            if group is None:
                group = Group(
                    id=group_id,
                    title=title,
                    enabled=enabled,
                    auto_grant_role=auto_grant_role,
                    allow_pm=allow_pm,
                )
                s.add(group)
            elif title is not None:
                group.title = title
            await s.commit()
            await s.refresh(group)
            return group

    async def update(self, group_id: int, **kwargs) -> Group | None:
        async with self._sf() as s:
            group = await s.get(Group, group_id)
            if group is None:
                return None
            for k, v in kwargs.items():
                setattr(group, k, v)
            await s.commit()
            await s.refresh(group)
            return group

    async def is_enabled(self, group_id: int) -> bool:
        group = await self.get(group_id)
        return group is not None and group.enabled

    async def get_thread_policy(self, group_id: int, thread_id: int | None) -> ThreadPolicy | None:
        async with self._sf() as s:
            result = await s.execute(
                select(ThreadPolicy).where(
                    ThreadPolicy.group_id == group_id,
                    ThreadPolicy.thread_id == thread_id,
                )
            )
            return result.scalar_one_or_none()

    async def set_thread_policy(self, group_id: int, thread_id: int | None, enabled: bool) -> ThreadPolicy:
        async with self._sf() as s:
            result = await s.execute(
                select(ThreadPolicy).where(
                    ThreadPolicy.group_id == group_id, ThreadPolicy.thread_id == thread_id
                )
            )
            policy = result.scalar_one_or_none()
            if policy is None:
                policy = ThreadPolicy(group_id=group_id, thread_id=thread_id, enabled=enabled)
                s.add(policy)
            else:
                policy.enabled = enabled
            await s.commit()
            await s.refresh(policy)
            return policy

    async def upsert_thread_name(self, group_id: int, thread_id: int, name: str) -> None:
        async with self._sf() as s:
            result = await s.execute(
                select(ThreadPolicy).where(
                    ThreadPolicy.group_id == group_id, ThreadPolicy.thread_id == thread_id
                )
            )
            policy = result.scalar_one_or_none()
            if policy is None:
                s.add(ThreadPolicy(group_id=group_id, thread_id=thread_id, name=name, enabled=True))
            else:
                policy.name = name
            await s.commit()

    async def delete_thread_policy(self, group_id: int, thread_id: int | None) -> bool:
        async with self._sf() as s:
            result = await s.execute(
                delete(ThreadPolicy).where(
                    ThreadPolicy.group_id == group_id, ThreadPolicy.thread_id == thread_id
                )
            )
            await s.commit()
            return result.rowcount > 0

    async def list_thread_policies(self, group_id: int) -> Sequence[ThreadPolicy]:
        async with self._sf() as s:
            result = await s.execute(
                select(ThreadPolicy).where(ThreadPolicy.group_id == group_id)
            )
            return result.scalars().all()

    async def is_thread_allowed(self, group_id: int, thread_id: int | None) -> bool:
        group = await self.get(group_id)
        if group is None:
            return False
        policy = await self.get_thread_policy(group_id, thread_id)
        if policy is not None:
            return policy.enabled
        all_policies = await self.list_thread_policies(group_id)
        has_whitelist = any(p.enabled for p in all_policies)
        return not has_whitelist

    async def get_storage(self, group_id: int) -> tuple[int | None, int | None]:
        """Return (storage_chat_id, storage_thread_id) for a group, or (None, None)."""
        group = await self.get(group_id)
        if group is None:
            return None, None
        return group.storage_chat_id, group.storage_thread_id

    async def set_storage(
        self,
        group_id: int,
        storage_chat_id: int | None,
        storage_thread_id: int | None,
    ) -> None:
        async with self._sf() as s:
            group = await s.get(Group, group_id)
            if group is None:
                return
            group.storage_chat_id = storage_chat_id
            group.storage_thread_id = storage_thread_id
            await s.commit()

    async def touch_member(self, group_id: int, user_id: int) -> None:
        """Ensure a UserGroupPolicy row exists for this (group, user) pair.

        Creates a bare row with no overrides if not yet present.
        This allows refresh_member_commands to find the group via
        UserGroupPolicy when iterating a user's groups.
        """
        async with self._sf() as s:
            result = await s.execute(
                select(UserGroupPolicy).where(
                    UserGroupPolicy.user_id == user_id,
                    UserGroupPolicy.group_id == group_id,
                )
            )
            if result.scalar_one_or_none() is None:
                s.add(UserGroupPolicy(user_id=user_id, group_id=group_id))
                await s.commit()
