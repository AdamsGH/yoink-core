"""CLI entry for the API service (runs uvicorn programmatically)."""
from __future__ import annotations

import uvicorn

from yoink.core.config import CoreSettings


def main() -> None:
    config = CoreSettings()
    uvicorn.run(
        "yoink.api_main:app",
        host="0.0.0.0",
        port=config.api_port,
        reload=config.debug,
        log_level="info",
    )
