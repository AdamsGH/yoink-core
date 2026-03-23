"""Tests for RBAC: role_gte, AccessPolicy, PermissionChecker, API role guards."""
from __future__ import annotations

import pytest

from tests.conftest import API_SECRET, OWNER_ID, make_jwt
from yoink.core.db.models import UserRole


class TestRoleGte:
    """Unit tests for role_gte() - no DB needed."""

    def test_owner_gte_all(self):
        from yoink.core.bot.access import role_gte
        for role in UserRole:
            assert role_gte(UserRole.owner, role) is True

    def test_user_not_gte_admin(self):
        from yoink.core.bot.access import role_gte
        assert role_gte(UserRole.user, UserRole.admin) is False

    def test_admin_gte_moderator(self):
        from yoink.core.bot.access import role_gte
        assert role_gte(UserRole.admin, UserRole.moderator) is True

    def test_banned_not_gte_user(self):
        from yoink.core.bot.access import role_gte
        assert role_gte(UserRole.banned, UserRole.user) is False

    def test_same_role(self):
        from yoink.core.bot.access import role_gte
        for role in UserRole:
            assert role_gte(role, role) is True


class TestAccessPolicyDefaults:
    """Unit tests for AccessPolicy default values."""

    def test_defaults(self):
        from yoink.core.bot.access import AccessPolicy
        policy = AccessPolicy()
        assert policy.min_role == UserRole.user
        assert policy.scopes == ["all"]
        assert policy.check_group_enabled is True
        assert policy.check_thread_policy is True
        assert policy.silent_deny is True
        assert policy.log_deny is True

    def test_admin_policy(self):
        from yoink.core.bot.access import AccessPolicy
        policy = AccessPolicy(min_role=UserRole.admin, silent_deny=False)
        assert policy.min_role == UserRole.admin
        assert policy.silent_deny is False


class TestPermissionChecker:
    """Tests for PermissionChecker.check() with mocked bot_data."""

    @pytest.mark.asyncio
    async def test_blocked_user_denied(self, session_factory, banned_user):
        from unittest.mock import MagicMock, AsyncMock
        from yoink.core.bot.access import AccessPolicy, PermissionChecker
        from yoink.core.db.repos.users import UserRepo

        checker = PermissionChecker()
        user_repo = UserRepo(session_factory, owner_id=OWNER_ID)

        ctx = MagicMock()
        ctx.bot_data = {"user_repo": user_repo}

        result = await checker.check(
            user_id=banned_user.id,
            chat=None,
            thread_id=None,
            policy=AccessPolicy(),
            context=ctx,
        )
        assert result.allowed is False
        assert result.deny_reason == "blocked"

    @pytest.mark.asyncio
    async def test_user_allowed(self, session_factory, regular_user):
        from unittest.mock import MagicMock
        from yoink.core.bot.access import AccessPolicy, PermissionChecker
        from yoink.core.db.repos.users import UserRepo

        checker = PermissionChecker()
        user_repo = UserRepo(session_factory, owner_id=OWNER_ID)

        ctx = MagicMock()
        ctx.bot_data = {"user_repo": user_repo}

        result = await checker.check(
            user_id=regular_user.id,
            chat=None,
            thread_id=None,
            policy=AccessPolicy(),
            context=ctx,
        )
        assert result.allowed is True

    @pytest.mark.asyncio
    async def test_user_below_admin_role(self, session_factory, regular_user):
        from unittest.mock import MagicMock
        from yoink.core.bot.access import AccessPolicy, PermissionChecker
        from yoink.core.db.repos.users import UserRepo

        checker = PermissionChecker()
        user_repo = UserRepo(session_factory, owner_id=OWNER_ID)

        ctx = MagicMock()
        ctx.bot_data = {"user_repo": user_repo}

        result = await checker.check(
            user_id=regular_user.id,
            chat=None,
            thread_id=None,
            policy=AccessPolicy(min_role=UserRole.admin),
            context=ctx,
        )
        assert result.allowed is False
        assert "role_user_below_admin" in result.deny_reason

    @pytest.mark.asyncio
    async def test_scope_private_only(self, session_factory, regular_user):
        from unittest.mock import MagicMock
        from yoink.core.bot.access import AccessPolicy, PermissionChecker
        from yoink.core.db.repos.users import UserRepo

        checker = PermissionChecker()
        user_repo = UserRepo(session_factory, owner_id=OWNER_ID)

        ctx = MagicMock()
        ctx.bot_data = {"user_repo": user_repo}

        mock_chat = MagicMock()
        mock_chat.type = "supergroup"
        mock_chat.id = -1001000099

        result = await checker.check(
            user_id=regular_user.id,
            chat=mock_chat,
            thread_id=None,
            policy=AccessPolicy(
                scopes=["private"],
                check_group_enabled=False,
                check_thread_policy=False,
            ),
            context=ctx,
        )
        assert result.allowed is False
        assert "scope_group_not_allowed" in result.deny_reason

    @pytest.mark.asyncio
    async def test_group_not_enabled(self, session_factory, regular_user):
        from unittest.mock import MagicMock
        from yoink.core.bot.access import AccessPolicy, PermissionChecker
        from yoink.core.db.repos.users import UserRepo
        from yoink.core.db.repos.groups import GroupRepo
        from yoink.core.db.models import Group

        async with session_factory() as sess:
            disabled_group = Group(id=-1001000099, title="Disabled", enabled=False)
            sess.add(disabled_group)
            await sess.commit()

        checker = PermissionChecker()
        user_repo = UserRepo(session_factory, owner_id=OWNER_ID)
        group_repo = GroupRepo(session_factory)

        ctx = MagicMock()
        ctx.bot_data = {"user_repo": user_repo, "group_repo": group_repo}

        mock_chat = MagicMock()
        mock_chat.type = "supergroup"
        mock_chat.id = -1001000099
        mock_chat.title = "Disabled"

        result = await checker.check(
            user_id=regular_user.id,
            chat=mock_chat,
            thread_id=None,
            policy=AccessPolicy(),
            context=ctx,
        )
        assert result.allowed is False
        assert "group_not_enabled" in result.deny_reason

        async with session_factory() as sess:
            g = await sess.get(Group, -1001000099)
            if g:
                await sess.delete(g)
                await sess.commit()


class TestAPIRoleGuards:
    """Test API endpoint role enforcement."""

    @pytest.mark.asyncio
    async def test_admin_endpoint_rejects_regular_user(self, api_client, regular_user):
        token = make_jwt(regular_user.id)
        resp = await api_client.get(
            "/api/v1/users",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_admin_endpoint_accepts_admin(self, api_client, admin):
        token = make_jwt(admin.id)
        resp = await api_client.get(
            "/api/v1/users",
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 200

    @pytest.mark.asyncio
    async def test_owner_role_cannot_be_assigned(self, api_client, owner, regular_user):
        token = make_jwt(owner.id)
        resp = await api_client.patch(
            f"/api/v1/users/{regular_user.id}",
            json={"role": "owner"},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403

    @pytest.mark.asyncio
    async def test_admin_cannot_change_admin_role(self, api_client, admin, owner):
        token = make_jwt(admin.id)
        resp = await api_client.patch(
            f"/api/v1/users/{owner.id}",
            json={"role": "user"},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403
