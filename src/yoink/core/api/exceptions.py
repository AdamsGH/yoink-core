"""Typed API exception hierarchy."""
from __future__ import annotations

from fastapi import HTTPException, status


class APIException(HTTPException):
    status_code: int = status.HTTP_400_BAD_REQUEST
    detail: str = "Bad request"

    def __init__(self, detail: str | None = None):
        super().__init__(status_code=self.status_code, detail=detail or self.detail)


class NotFoundError(APIException):
    status_code = status.HTTP_404_NOT_FOUND
    detail = "Not found"


class ForbiddenError(APIException):
    status_code = status.HTTP_403_FORBIDDEN
    detail = "Forbidden"


class UnauthorizedError(APIException):
    status_code = status.HTTP_401_UNAUTHORIZED
    detail = "Unauthorized"


class ConflictError(APIException):
    status_code = status.HTTP_409_CONFLICT
    detail = "Conflict"
