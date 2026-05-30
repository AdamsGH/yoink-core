"""PTB Application factory."""
from __future__ import annotations

from typing import TYPE_CHECKING

from telegram.ext import Application
from telegram.request import HTTPXRequest

if TYPE_CHECKING:
    from yoink.core.config import CoreSettings


def create_bot_app(config: CoreSettings) -> Application:
    # Main client: serves bot API calls including media uploads. Pool is sized
    # for concurrent_updates plus large uploads holding a connection for minutes.
    # media_write_timeout covers sendVideo/sendDocument; per-call write_timeout
    # passed by callers still wins via HTTPXRequest.do_request.
    request = HTTPXRequest(
        http_version="1.1",
        connection_pool_size=64,
        connect_timeout=10.0,
        read_timeout=30.0,
        write_timeout=60.0,
        pool_timeout=30.0,
        media_write_timeout=600.0,
    )
    # Dedicated client for long-poll getUpdates so a stuck upload on the main
    # pool cannot starve the polling loop. Long-poll read must exceed the
    # configured long-poll timeout (PTB defaults to 10s; keep margin).
    get_updates_request = HTTPXRequest(
        http_version="1.1",
        connection_pool_size=4,
        connect_timeout=10.0,
        read_timeout=40.0,
        write_timeout=10.0,
        pool_timeout=10.0,
    )
    builder = (
        Application.builder()
        .token(config.bot_token)
        .request(request)
        .get_updates_request(get_updates_request)
        .concurrent_updates(True)
    )
    if config.telegram_base_url != "https://api.telegram.org/bot":
        builder = builder.base_url(config.telegram_base_url)
    return builder.build()
