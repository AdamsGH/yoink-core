"""API key generation and verification for M2M auth."""
from __future__ import annotations

import hashlib
import secrets
from datetime import datetime, timezone


PREFIX_LEN = 8


def generate_api_key() -> tuple[str, str, str]:
    """Generate a new API key.

    Returns (raw_key, key_hash, prefix).
    raw_key is shown to the user once, key_hash is stored in DB.
    """
    raw = f"yoink_{secrets.token_urlsafe(32)}"
    key_hash = hashlib.sha256(raw.encode()).hexdigest()
    prefix = raw[:PREFIX_LEN]
    return raw, key_hash, prefix


def hash_key(raw: str) -> str:
    """Hash a raw API key for comparison."""
    return hashlib.sha256(raw.encode()).hexdigest()


def is_expired(expires_at: datetime | None) -> bool:
    """Check whether the key has expired."""
    if expires_at is None:
        return False
    exp = expires_at if expires_at.tzinfo else expires_at.replace(tzinfo=timezone.utc)
    return exp <= datetime.now(timezone.utc)


def has_scope(key_scopes: list[str], required: str) -> bool:
    """Check whether key_scopes satisfy the required scope.

    Supports wildcard: ["*"] grants everything.
    Supports category wildcard: "users:*" grants "users:read", "users:write", etc.
    """
    if "*" in key_scopes:
        return True
    if required in key_scopes:
        return True
    category = required.split(":")[0]
    if f"{category}:*" in key_scopes:
        return True
    return False


ALL_SCOPES = [
    "users:read",
    "users:write",
    "groups:read",
    "groups:write",
    "downloads:read",
    "settings:read",
    "settings:write",
    "events:read",
    "events:write",
    "health:read",
]
