"""PTB Application factory."""
from __future__ import annotations

import httpx
from telegram.ext import Application
from telegram.request import HTTPXRequest

from yoink.core.config import CoreSettings


def create_bot_app(config: CoreSettings) -> Application:
    request = HTTPXRequest(
        http_version="1.1",
        connection_pool_size=8,
        httpx_kwargs={
            "timeout": httpx.Timeout(connect=10.0, read=30.0, write=60.0, pool=5.0),
        },
    )
    builder = Application.builder().token(config.bot_token).request(request)
    if config.telegram_base_url != "https://api.telegram.org/bot":
        builder = builder.base_url(config.telegram_base_url)
    return builder.build()
