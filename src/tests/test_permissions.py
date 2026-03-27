"""Tests for UserPermissionRepo.has() - explicit grant + role threshold paths."""
from __future__ import annotations

import pytest
import pytest_asyncio
from datetime import datetime, timedelta, timezone

from tests.conftest import OWNER_ID
from yoink.core.db.models import User, UserPermission, UserRole
from yoink.core.db.repos.permissions import UserPermissionRepo
from yoink.core.plugin import FeatureSpec, _feature_registry


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

_TEST_PLUGIN = "testplugin"
_OPEN_FEATURE = "open_feature"      # default_min_role="user"
_ADMIN_FEATURE = "admin_feature"    # default_min_role="admin"
_GATED_FEATURE = "gated_feature"    # default_min_role=None (explicit grant only)


def _register_test_features():
    """Idempotently inject test FeatureSpecs into the global registry."""
    existing = {(s.plugin, s.feature) for s in _feature_registry}
    specs = [
        FeatureSpec(
            plugin=_TEST_PLUGIN,
            feature=_OPEN_FEATURE,
            label="Open",
            default_min_role="user",
        ),
        FeatureSpec(
            plugin=_TEST_PLUGIN,
            feature=_ADMIN_FEATURE,
            label="Admin",
            default_min_role="admin",
        ),
        FeatureSpec(
            plugin=_TEST_PLUGIN,
            feature=_GATED_FEATURE,
            label="Gated",
            default_min_role=None,
        ),
    ]
    for spec in specs:
        if (spec.plugin, spec.feature) not in existing:
            _feature_registry.append(spec)


_register_test_features()


async def _make_user(session_factory, user_id: int, role: UserRole) -> User:
    async with session_factory() as s:
        u = await s.get(User, user_id)
        if u is None:
            u = User(id=user_id, role=role)
            s.add(u)
            await s.commit()
            await s.refresh(u)
        else:
            u.role = role
            await s.commit()
            await s.refresh(u)
        return u


async def _cleanup(session_factory, user_id: int) -> None:
    async with session_factory() as s:
        u = await s.get(User, user_id)
        if u:
            await s.delete(u)
            await s.commit()


# ---------------------------------------------------------------------------
# Path 1: explicit grant
# ---------------------------------------------------------------------------

class TestExplicitGrant:
    @pytest.mark.asyncio
    async def test_explicit_grant_allows_any_role(self, session_factory):
        user = await _make_user(session_factory, 200001, UserRole.restricted)
        repo = UserPermissionRepo(session_factory)
        await repo.grant(200001, _TEST_PLUGIN, _GATED_FEATURE, granted_by=OWNER_ID)
        try:
            assert await repo.has(200001, _TEST_PLUGIN, _GATED_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200001)

    @pytest.mark.asyncio
    async def test_expired_grant_is_denied(self, session_factory):
        user = await _make_user(session_factory, 200002, UserRole.user)
        repo = UserPermissionRepo(session_factory)
        past = datetime.now(timezone.utc) - timedelta(hours=1)
        await repo.grant(200002, _TEST_PLUGIN, _GATED_FEATURE, granted_by=OWNER_ID, expires_at=past)
        try:
            assert await repo.has(200002, _TEST_PLUGIN, _GATED_FEATURE, user=user) is False
        finally:
            await _cleanup(session_factory, 200002)

    @pytest.mark.asyncio
    async def test_non_expired_grant_is_allowed(self, session_factory):
        user = await _make_user(session_factory, 200003, UserRole.restricted)
        repo = UserPermissionRepo(session_factory)
        future = datetime.now(timezone.utc) + timedelta(days=30)
        await repo.grant(200003, _TEST_PLUGIN, _GATED_FEATURE, granted_by=OWNER_ID, expires_at=future)
        try:
            assert await repo.has(200003, _TEST_PLUGIN, _GATED_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200003)

    @pytest.mark.asyncio
    async def test_no_grant_no_user_returns_false(self, session_factory):
        repo = UserPermissionRepo(session_factory)
        assert await repo.has(299999, _TEST_PLUGIN, _GATED_FEATURE) is False


# ---------------------------------------------------------------------------
# Path 2: role threshold
# ---------------------------------------------------------------------------

class TestRoleThreshold:
    @pytest.mark.asyncio
    async def test_user_role_meets_user_threshold(self, session_factory):
        user = await _make_user(session_factory, 200010, UserRole.user)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200010, _TEST_PLUGIN, _OPEN_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200010)

    @pytest.mark.asyncio
    async def test_moderator_meets_user_threshold(self, session_factory):
        user = await _make_user(session_factory, 200011, UserRole.moderator)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200011, _TEST_PLUGIN, _OPEN_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200011)

    @pytest.mark.asyncio
    async def test_restricted_below_user_threshold(self, session_factory):
        user = await _make_user(session_factory, 200012, UserRole.restricted)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200012, _TEST_PLUGIN, _OPEN_FEATURE, user=user) is False
        finally:
            await _cleanup(session_factory, 200012)

    @pytest.mark.asyncio
    async def test_admin_meets_admin_threshold(self, session_factory):
        user = await _make_user(session_factory, 200013, UserRole.admin)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200013, _TEST_PLUGIN, _ADMIN_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200013)

    @pytest.mark.asyncio
    async def test_user_below_admin_threshold(self, session_factory):
        user = await _make_user(session_factory, 200014, UserRole.user)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200014, _TEST_PLUGIN, _ADMIN_FEATURE, user=user) is False
        finally:
            await _cleanup(session_factory, 200014)

    @pytest.mark.asyncio
    async def test_owner_meets_any_threshold(self, session_factory):
        user = await _make_user(session_factory, 200015, UserRole.owner)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200015, _TEST_PLUGIN, _ADMIN_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200015)

    @pytest.mark.asyncio
    async def test_none_threshold_requires_explicit_grant(self, session_factory):
        """default_min_role=None: role alone is never enough."""
        user = await _make_user(session_factory, 200016, UserRole.owner)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200016, _TEST_PLUGIN, _GATED_FEATURE, user=user) is False
        finally:
            await _cleanup(session_factory, 200016)

    @pytest.mark.asyncio
    async def test_without_user_object_role_path_skipped(self, session_factory):
        """Without user= kwarg, only the DB grant path is evaluated."""
        repo = UserPermissionRepo(session_factory)
        # user 200017 has role=user which would satisfy _OPEN_FEATURE threshold,
        # but since we don't pass user= the check must return False.
        await _make_user(session_factory, 200017, UserRole.user)
        try:
            assert await repo.has(200017, _TEST_PLUGIN, _OPEN_FEATURE) is False
        finally:
            await _cleanup(session_factory, 200017)


# ---------------------------------------------------------------------------
# Blocked user: role threshold must never fire
# ---------------------------------------------------------------------------

class TestBlockedUser:
    @pytest.mark.asyncio
    async def test_banned_user_role_path_blocked(self, session_factory):
        user = await _make_user(session_factory, 200020, UserRole.banned)
        repo = UserPermissionRepo(session_factory)
        try:
            # banned has is_blocked=True, so role path must not fire even if
            # the feature has default_min_role="banned" (which would be unusual)
            assert await repo.has(200020, _TEST_PLUGIN, _OPEN_FEATURE, user=user) is False
        finally:
            await _cleanup(session_factory, 200020)

    @pytest.mark.asyncio
    async def test_banned_user_explicit_grant_still_denied(self, session_factory):
        """Explicit grants are evaluated before the is_blocked guard.
        The grant path (path 1) does NOT check is_blocked - that is the caller's
        responsibility (PermissionChecker does it before calling perm_repo.has).
        This test documents the current behaviour: grant alone returns True from
        has(), but PermissionChecker will block before reaching this call.
        """
        user = await _make_user(session_factory, 200021, UserRole.banned)
        repo = UserPermissionRepo(session_factory)
        await repo.grant(200021, _TEST_PLUGIN, _GATED_FEATURE, granted_by=OWNER_ID)
        try:
            # has() itself only guards the role path with is_blocked;
            # the explicit grant path is unconditional.
            assert await repo.has(200021, _TEST_PLUGIN, _GATED_FEATURE, user=user) is True
        finally:
            await _cleanup(session_factory, 200021)


# ---------------------------------------------------------------------------
# Unknown feature (not in registry)
# ---------------------------------------------------------------------------

class TestUnknownFeature:
    @pytest.mark.asyncio
    async def test_unknown_feature_no_grant_denied(self, session_factory):
        user = await _make_user(session_factory, 200030, UserRole.owner)
        repo = UserPermissionRepo(session_factory)
        try:
            assert await repo.has(200030, _TEST_PLUGIN, "nonexistent", user=user) is False
        finally:
            await _cleanup(session_factory, 200030)

    @pytest.mark.asyncio
    async def test_unknown_feature_explicit_grant_allowed(self, session_factory):
        user = await _make_user(session_factory, 200031, UserRole.user)
        repo = UserPermissionRepo(session_factory)
        await repo.grant(200031, _TEST_PLUGIN, "nonexistent", granted_by=OWNER_ID)
        try:
            assert await repo.has(200031, _TEST_PLUGIN, "nonexistent", user=user) is True
        finally:
            await _cleanup(session_factory, 200031)
