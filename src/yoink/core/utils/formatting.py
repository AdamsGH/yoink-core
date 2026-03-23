"""Formatting utilities."""
from __future__ import annotations

import re
from datetime import timedelta


def format_size(size_bytes: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size_bytes < 1024:
            return f"{size_bytes:.1f} {unit}"
        size_bytes //= 1024
    return f"{size_bytes:.1f} TB"


def escape_html(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def humantime(milliseconds: int | float) -> str:
    """Convert milliseconds to a human-readable duration string.

    Examples: 3723000 -> '1h 2m 3s', 45000 -> '45s'
    """
    seconds = int(milliseconds / 1000)
    if seconds < 60:
        return f"{seconds}s"
    minutes, sec = divmod(seconds, 60)
    if minutes < 60:
        return f"{minutes}m {sec}s"
    hours, mins = divmod(minutes, 60)
    return f"{hours}h {mins}m {sec}s"


def parse_duration(s: str) -> timedelta | None:
    """Parse a compact duration string into a timedelta.

    Accepted units: s, m, h, d. Combinable: '2h30m', '1d 12h', '90s'.
    Returns None if the string cannot be parsed or the result is zero.
    """
    total = timedelta()
    units = {"s": 1, "m": 60, "h": 3600, "d": 86400}
    for val, unit in re.findall(r"(\d+)\s*([smhd])", s.lower()):
        total += timedelta(seconds=int(val) * units[unit])
    return total if total.total_seconds() > 0 else None
