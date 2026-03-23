"""Logging configuration: JSON formatter for production, human-readable for dev."""
from __future__ import annotations

import json
import logging
import sys
from datetime import datetime, timezone


class JsonFormatter(logging.Formatter):
    """Outputs one JSON object per line, suitable for log aggregation."""

    def format(self, record: logging.LogRecord) -> str:
        entry: dict = {
            "ts": datetime.fromtimestamp(record.created, tz=timezone.utc).isoformat(),
            "level": record.levelname.lower(),
            "logger": record.name,
            "msg": record.getMessage(),
        }
        if record.exc_info and record.exc_info[1] is not None:
            entry["exc"] = self.formatException(record.exc_info)
        if hasattr(record, "extra_data"):
            entry["data"] = record.extra_data
        return json.dumps(entry, ensure_ascii=False, default=str)


def setup_logging(*, debug: bool = False, json_logs: bool = False) -> None:
    level = logging.DEBUG if debug else logging.INFO
    root = logging.getLogger()
    root.setLevel(level)

    # Remove any existing handlers (idempotent re-configuration)
    for h in root.handlers[:]:
        root.removeHandler(h)

    handler = logging.StreamHandler(sys.stdout)
    if json_logs:
        handler.setFormatter(JsonFormatter())
    else:
        handler.setFormatter(logging.Formatter(
            "%(asctime)s %(name)s %(levelname)s %(message)s",
        ))
    root.addHandler(handler)

    # Quieten noisy libraries
    for noisy in (
        "httpx", "httpcore", "telegram", "apscheduler",
        "asyncio", "urllib3", "uvicorn.access",
    ):
        logging.getLogger(noisy).setLevel(logging.WARNING)
