"""Standardized response helpers."""
from __future__ import annotations

from typing import Any

from fastapi.responses import JSONResponse


def success_response(data: Any, status_code: int = 200) -> JSONResponse:
    return JSONResponse(content={"ok": True, "data": data}, status_code=status_code)


def error_response(detail: str, status_code: int = 400) -> JSONResponse:
    return JSONResponse(content={"ok": False, "error": detail}, status_code=status_code)


def paginated_response(items: list, total: int, offset: int, limit: int) -> dict:
    return {
        "items": items,
        "total": total,
        "offset": offset,
        "limit": limit,
        "has_more": offset + limit < total,
    }
