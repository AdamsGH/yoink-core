"""Core API request/response schemas."""
from __future__ import annotations

from datetime import datetime

from pydantic import BaseModel

from yoink.core.db.models import UserRole

# CoreSettingsResponse was removed (it was empty). Use settings.SettingsResponse instead.


class TelegramInitDataRequest(BaseModel):
    """Full initData string from Telegram WebApp (HMAC-signed)."""
    init_data: str
    # Fallback fields used only when init_data is absent (dev/testing).
    # Production auth must always include init_data.
    telegram_id: int | None = None
    username: str | None = None
    first_name: str | None = None


class TokenResponse(BaseModel):
    access_token: str
    token_type: str = "bearer"
    user_id: int
    role: str


class UserResponse(BaseModel):
    id: int
    username: str | None
    first_name: str | None
    role: UserRole
    theme: str
    created_at: datetime
    updated_at: datetime


class UserUpdateRequest(BaseModel):
    role: UserRole | None = None
    ban_until: datetime | None = None


class ThreadPolicyInline(BaseModel):
    id: int
    thread_id: int | None
    name: str | None
    enabled: bool


class GroupResponse(BaseModel):
    id: int
    title: str | None
    enabled: bool
    auto_grant_role: UserRole
    allow_pm: bool
    nsfw_allowed: bool
    storage_chat_id: int | None = None
    storage_thread_id: int | None = None
    created_at: datetime
    thread_policies: list[ThreadPolicyInline] = []


class PaginatedResponse(BaseModel):
    total: int
    offset: int
    limit: int
    has_more: bool
