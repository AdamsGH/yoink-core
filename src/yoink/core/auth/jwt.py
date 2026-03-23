"""JWT utilities."""
from __future__ import annotations

from datetime import datetime, timedelta, timezone

from fastapi import HTTPException, status
from jose import JWTError, jwt

ALGORITHM = "HS256"


def create_access_token(
    user_id: int,
    role: str,
    secret: str,
    expires_minutes: int,
    first_name: str | None = None,
    username: str | None = None,
) -> str:
    expire = datetime.now(timezone.utc) + timedelta(minutes=expires_minutes)
    payload: dict = {"sub": str(user_id), "role": role, "exp": expire}
    if first_name:
        payload["first_name"] = first_name
    if username:
        payload["username"] = username
    return jwt.encode(payload, secret, algorithm=ALGORITHM)


def verify_token(token: str, secret: str) -> dict:
    try:
        return jwt.decode(token, secret, algorithms=[ALGORITHM])
    except JWTError as exc:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid or expired token",
            headers={"WWW-Authenticate": "Bearer"},
        ) from exc
