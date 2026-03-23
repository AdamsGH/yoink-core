"""
Combined entrypoint: runs PTB bot + FastAPI/uvicorn in a single asyncio event loop.

Both share the same session_factory, bot_data, and plugin instances - no HTTP
overhead for internal calls between bot and API layers.
"""
from __future__ import annotations

import asyncio
import logging
import signal

import uvicorn
from telegram import Update

from yoink.app import build_app
from yoink.core.api.app import create_api
from yoink.core.config import CoreSettings
from yoink.core.plugin import load_plugins

logger = logging.getLogger(__name__)


def main() -> None:
    config = CoreSettings()

    from yoink.core.logging import setup_logging
    setup_logging(debug=config.debug, json_logs=config.json_logs)

    asyncio.run(_run(config))


async def _run(config: CoreSettings) -> None:
    plugins = load_plugins(config.yoink_plugins)

    ptb_app = build_app(config=config, plugins=plugins)
    api_app = create_api(config, plugins=plugins)

    # Wire the PTB bot into FastAPI app.state so API routers can call the bot
    # directly without HTTP round-trips (e.g. send_message from admin endpoints).
    async with ptb_app:
        await ptb_app.initialize()

        # post_init is only called by run_polling/run_webhook, not by initialize().
        # Call it manually so bot_data is populated before we try to read it.
        if ptb_app.post_init:
            await ptb_app.post_init(ptb_app)

        # Share the session_factory and bot_data that PTB post_init populated.
        api_app.state.settings = config
        api_app.state.session_factory = ptb_app.bot_data["session_factory"]
        api_app.state.bot_data = ptb_app.bot_data
        api_app.state.bot = ptb_app.bot

        uv_config = uvicorn.Config(
            api_app,
            host="0.0.0.0",
            port=config.api_port,
            loop="none",
            log_level="warning",
            access_log=False,
        )
        server = uvicorn.Server(uv_config)

        stop_event = asyncio.Event()

        def _handle_signal(sig: int) -> None:
            logger.info("Signal %s received - shutting down", sig)
            stop_event.set()

        loop = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, _handle_signal, sig)

        await ptb_app.start()

        polling_task = asyncio.create_task(
            ptb_app.updater.start_polling(
                allowed_updates=Update.ALL_TYPES,
                drop_pending_updates=True,
                bootstrap_retries=-1,
                poll_interval=0.5,
                timeout=10,
            ),
            name="ptb-polling",
        )
        server_task = asyncio.create_task(server.serve(), name="uvicorn")

        logger.info(
            "Combined service started - bot + API on :%d | plugins: %s",
            config.api_port,
            [p.name for p in plugins],
        )

        await stop_event.wait()

        logger.info("Stopping uvicorn...")
        server.should_exit = True
        await server_task

        logger.info("Stopping PTB polling...")
        await ptb_app.updater.stop()
        polling_task.cancel()
        try:
            await polling_task
        except asyncio.CancelledError:
            pass

        await ptb_app.stop()
        await ptb_app.shutdown()
        logger.info("Shutdown complete")
