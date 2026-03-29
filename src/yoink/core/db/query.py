"""SQL query utilities shared across plugins."""
from __future__ import annotations

from pathlib import Path


def load_sql(base: Path, name: str) -> str:
    """Load a .sql file relative to *base* directory.

    Typical usage in a plugin::

        from pathlib import Path
        from yoink.core.db.query import load_sql

        _Q = Path(__file__).parent / "queries"
        _TOP_USERS = load_sql(_Q, "top_users")

    The file is read once at import time and cached as a module-level string.
    """
    return (base / f"{name}.sql").read_text(encoding="utf-8")


def date_condition(col: str = "date") -> str:
    """Return a SQL fragment that filters *col* by an optional :since parameter.

    Works with both named-parameter drivers (asyncpg via SQLAlchemy) and avoids
    f-string injection.  Pass ``None`` for ``:since`` to disable the filter::

        WHERE chat_id = :chat_id
          AND {date_condition()}

    The generated fragment uses a nullable-parameter trick so the query text
    is always identical regardless of whether a date window is requested.
    This means the query plan can be cached by PostgreSQL.
    """
    return f"(CAST(:since AS timestamptz) IS NULL OR {col} >= :since)"


def date_params(since, **extra) -> dict:
    """Build a parameter dict that always includes ``since`` (possibly ``None``).

    Combine with :func:`date_condition` so the query text never changes::

        params = date_params(since, chat_id=chat_id, limit=limit)
        rows = await session.execute(text(sql), params)
    """
    return {"since": since, **extra}
