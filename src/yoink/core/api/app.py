"""FastAPI application factory."""
from __future__ import annotations

from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING

import time

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from scalar_fastapi import get_scalar_api_reference
from starlette.types import ASGIApp, Receive, Scope, Send

from yoink.core.api.health import router as health_router
from yoink.core.api.internal.router import router as internal_router
from yoink.core.api.routers import api_keys, auth, bot_settings, forum, groups, messages, permissions, settings, threads, users
from yoink.core.db.engine import create_tables, get_session_factory, init_engine

if TYPE_CHECKING:
    from yoink.core.plugin import YoinkPlugin


def create_api(config, plugins: list["YoinkPlugin"] | None = None) -> FastAPI:
    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncGenerator[None, None]:
        from pathlib import Path
        from yoink.core.services.user_session import UserSessionService

        init_engine(config.database_url, echo=config.database_echo)
        if plugins:
            for plugin in plugins:
                plugin.get_models()
        await create_tables()
        app.state.settings = config
        app.state.session_factory = get_session_factory()

        # Expose UserSessionService via app.state so forum/messages/threads routers
        # can reach it without bot_data (API process has no PTB Application).
        app.state.user_session = UserSessionService(
            base_url=config.telegram_base_url.replace("/bot", ""),
            token_file=Path(str(config.data_dir)) / "tg-bot-api" / "user.token",
        )
        # In combined mode combined_main.py sets app.state.bot_data to the live
        # PTB bot_data dict before uvicorn starts. Only initialise it here when
        # running in API-only mode (no bot process sharing state).
        if not getattr(app.state, "bot_data", None):
            app.state.bot_data = {}
        app.state.bot_data.setdefault("user_session", app.state.user_session)
        yield

    app = FastAPI(
        title="Yoink API",
        version="1.0.0",
        description=(
            "REST API for the Yoink Telegram bot.\n\n"
            "## Authentication\n\n"
            "All endpoints (except `/health`, `POST /api/v1/auth/token`, and `POST /api/v1/dl/cookies/submit`) "
            "require a Bearer JWT.\n\n"
            "**How to get a token for Try-it:**\n"
            "1. Call `POST /api/v1/auth/dev` with your `user_id` — copy `access_token` from the response\n"
            "2. Click the auth button (🔑) in Scalar's top-right corner, paste the token\n"
            "3. All subsequent requests will include it automatically\n\n"
            "## Roles\n\n"
            "`owner > admin > moderator > user > restricted > banned`. "
            "Each endpoint documents the minimum role required in parentheses, e.g. *(admin+)*."
        ),
        docs_url=None,
        redoc_url=None,
        lifespan=lifespan,
        openapi_tags=[
            {
                "name": "auth",
                "description": "Obtain JWT tokens. Use `POST /token` with Telegram WebApp `initData`.",
            },
            {
                "name": "users",
                "description": "User profiles, roles, and personal statistics.",
            },
            {
                "name": "groups",
                "description": "Telegram group (chat) management: settings, thread policies, member overrides.",
            },
            {
                "name": "permissions",
                "description": "Feature-level RBAC: grant or revoke plugin features per user.",
            },
            {
                "name": "settings",
                "description": "Personal user settings: language, theme.",
            },
            {
                "name": "bot-settings",
                "description": "Global bot configuration: access mode, tag map, feature flags.",
            },
            {
                "name": "api-keys",
                "description": "Programmatic API keys for non-Telegram clients.",
            },
            {
                "name": "forum",
                "description": "Forum topic proxy via user-mode Telegram session (owner only).",
            },
            {
                "name": "threads",
                "description": "Thread (forum topic) download policies per group.",
            },
            {
                "name": "messages",
                "description": "Message-level proxy via user-mode Telegram session (owner only).",
            },
            {
                "name": "downloader",
                "description": "Download history, cookies, NSFW filter, and downloader settings.",
            },
            {
                "name": "insight",
                "description": "AI-powered video summary access control and per-user settings.",
            },
            {
                "name": "stats",
                "description": "Chat analytics: activity, top users, word frequency, member events, import.",
            },
        ],
    )

    # Inject global security into OpenAPI schema so Scalar shows auth UI
    # and pre-fills Bearer token for all protected endpoints.
    _original_openapi = app.openapi

    def _openapi_with_security():
        schema = _original_openapi()
        schema.setdefault("security", [{"HTTPBearer": []}, {"APIKeyHeader": []}])
        return schema

    app.openapi = _openapi_with_security  # type: ignore[method-assign]

    from yoink.core.metrics import metrics

    class MetricsMiddleware:
        """Pure ASGI middleware - no BaseHTTPMiddleware task spawning."""
        def __init__(self, app: ASGIApp) -> None:
            self.app = app

        async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
            if scope["type"] != "http" or scope["path"] in ("/health", "/metrics"):
                await self.app(scope, receive, send)
                return
            metrics.inc("api_requests_total")
            t0 = time.monotonic()
            status_code = 200

            async def send_wrapper(message) -> None:
                nonlocal status_code
                if message["type"] == "http.response.start":
                    status_code = message["status"]
                await send(message)

            await self.app(scope, receive, send_wrapper)
            metrics.observe("api_response_seconds", time.monotonic() - t0)
            if status_code >= 400:
                metrics.inc("api_errors_total")

    app.add_middleware(MetricsMiddleware)
    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_credentials=True,
        allow_methods=["*"],
        allow_headers=["*"],
    )

    app.include_router(health_router)
    app.include_router(auth.router, prefix="/api/v1")
    app.include_router(users.router, prefix="/api/v1")
    app.include_router(groups.router, prefix="/api/v1")
    app.include_router(settings.router, prefix="/api/v1")
    app.include_router(bot_settings.router, prefix="/api/v1")
    app.include_router(threads.router, prefix="/api/v1")
    app.include_router(forum.router, prefix="/api/v1")
    app.include_router(messages.router, prefix="/api/v1")
    app.include_router(api_keys.router, prefix="/api/v1")
    app.include_router(permissions.router, prefix="/api/v1")
    app.include_router(internal_router, prefix="/api/internal/v1")

    if plugins:
        for plugin in plugins:
            router = plugin.get_routes()
            if router is not None:
                app.include_router(router, prefix=f"/api/v1/{plugin.name}")

    @app.get("/docs", include_in_schema=False)
    async def scalar_html():
        return get_scalar_api_reference(
            openapi_url="/openapi.json",
            title="Yoink API",
            # Prefill HTTPBearer scheme so Try-it works without manual token entry.
            # The token field stays empty — user pastes their JWT once and it's reused.
            authentication={
                "preferredSecurityScheme": "HTTPBearer",
                "securitySchemes": {
                    "HTTPBearer": {
                        "type": "http",
                        "scheme": "bearer",
                        "bearerFormat": "JWT",
                        "token": "",
                    }
                },
            },
            persist_auth=True,
        )

    return app
