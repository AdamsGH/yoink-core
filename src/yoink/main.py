"""CLI entrypoints for yoink-core."""
from __future__ import annotations

from telegram import Update

from yoink.app import build_app
from yoink.core.config import CoreSettings


def main() -> None:
    config = CoreSettings()

    from yoink.core.logging import setup_logging
    setup_logging(debug=config.debug, json_logs=config.json_logs)

    app = build_app(config=config)
    app.run_polling(
        allowed_updates=Update.ALL_TYPES,
        drop_pending_updates=True,
        bootstrap_retries=-1,
        poll_interval=0.5,
        timeout=10,
    )
