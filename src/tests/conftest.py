"""Shared test fixtures: async DB engine, session, API client, test users."""
from __future__ import annotations

import os

import pytest
import pytest_asyncio
import sqlalchemy
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import (
    AsyncSession,
    async_sessionmaker,
    create_async_engine,
)

from yoink.core.db.base import Base
from yoink.core.db.models import User, UserRole, Group

TEST_DB_URL = os.getenv(
    "TEST_DATABASE_URL",
    "postgresql+asyncpg://yoink:yoink@yoink-postgres:5432/yoink_test",
)

BOT_TOKEN = "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
API_SECRET = "test-secret-key-for-jwt"
OWNER_ID = 100001


@pytest.fixture(scope="session")
def anyio_backend():
    return "asyncio"


@pytest_asyncio.fixture
async def db_engine():
    eng = create_async_engine(TEST_DB_URL, echo=False, pool_size=5, pool_recycle=300)
    async with eng.begin() as conn:
        await conn.execute(sqlalchemy.text("DROP SCHEMA public CASCADE"))
        await conn.execute(sqlalchemy.text("CREATE SCHEMA public"))
        await conn.run_sync(Base.metadata.create_all)
    yield eng
    await eng.dispose()


@pytest_asyncio.fixture
async def session_factory(db_engine):
    return async_sessionmaker(db_engine, expire_on_commit=False)


async def _create_user(session_factory, **kwargs) -> User:
    async with session_factory() as sess:
        existing = await sess.get(User, kwargs["id"])
        if existing:
            for k, v in kwargs.items():
                if k != "id":
                    setattr(existing, k, v)
            await sess.commit()
            await sess.refresh(existing)
            return existing
        user = User(**kwargs)
        sess.add(user)
        await sess.commit()
        await sess.refresh(user)
        return user


async def _delete_user(session_factory, user_id: int) -> None:
    async with session_factory() as sess:
        user = await sess.get(User, user_id)
        if user:
            await sess.delete(user)
            await sess.commit()


@pytest_asyncio.fixture
async def owner(session_factory):
    user = await _create_user(
        session_factory, id=OWNER_ID, username="owner",
        first_name="Owner", role=UserRole.owner,
    )
    yield user
    await _delete_user(session_factory, user.id)


@pytest_asyncio.fixture
async def admin(session_factory):
    user = await _create_user(
        session_factory, id=100002, username="admin",
        first_name="Admin", role=UserRole.admin,
    )
    yield user
    await _delete_user(session_factory, user.id)


@pytest_asyncio.fixture
async def regular_user(session_factory):
    user = await _create_user(
        session_factory, id=100003, username="regular",
        first_name="Regular", role=UserRole.user,
    )
    yield user
    await _delete_user(session_factory, user.id)


@pytest_asyncio.fixture
async def banned_user(session_factory):
    user = await _create_user(
        session_factory, id=100004, username="banned",
        first_name="Banned", role=UserRole.banned,
    )
    yield user
    await _delete_user(session_factory, user.id)


@pytest_asyncio.fixture
async def test_group(session_factory):
    async with session_factory() as sess:
        group = Group(id=-1001000001, title="Test Group", enabled=True)
        sess.add(group)
        await sess.commit()
        await sess.refresh(group)
    yield group
    async with session_factory() as sess:
        g = await sess.get(Group, group.id)
        if g:
            await sess.delete(g)
            await sess.commit()


def make_jwt(user_id: int, role: str = "user", secret: str = API_SECRET) -> str:
    from yoink.core.auth.jwt import create_access_token
    return create_access_token(user_id, role, secret, expires_minutes=60)


def _make_test_config():
    """Create a CoreSettings-like object for tests without reading .env."""
    from unittest.mock import MagicMock
    cfg = MagicMock()
    cfg.bot_token = BOT_TOKEN
    cfg.owner_id = OWNER_ID
    cfg.database_url = TEST_DB_URL
    cfg.database_echo = False
    cfg.api_port = 8000
    cfg.api_secret_key = API_SECRET
    cfg.api_token_expire_minutes = 1440
    cfg.debug = False
    cfg.dev_auth_enabled = False
    cfg.json_logs = False
    cfg.data_dir = "/tmp/yoink-test-data"
    cfg.default_language = "en"
    cfg.rate_limit_per_minute = 5
    cfg.rate_limit_per_hour = 30
    cfg.rate_limit_per_day = 100
    cfg.log_channel = None
    cfg.log_exception_channel = None
    cfg.telegram_base_url = "https://api.telegram.org/bot"
    cfg.yoink_plugins = ""
    return cfg


@pytest_asyncio.fixture
async def api_client(session_factory):
    """Async HTTP client wired to the FastAPI app with test DB."""
    from yoink.core.api.app import create_api

    config = _make_test_config()
    app = create_api(config, plugins=[])

    app.state.settings = config
    app.state.session_factory = session_factory
    app.state.bot_data = {}
    app.state.bot = None

    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        yield client
