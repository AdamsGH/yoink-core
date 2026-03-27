"""Core ORM models: User, Group, ThreadPolicy, UserGroupPolicy, BotSetting, Event, ApiKey."""
from __future__ import annotations

import enum
from datetime import datetime
from typing import Any

from sqlalchemy import (
    BigInteger, Boolean, DateTime, Enum, Float, Index,
    Integer, JSON, String, Text, UniqueConstraint, ForeignKey,
)
from sqlalchemy.orm import Mapped, mapped_column, relationship

from yoink.core.db.base import Base, _now


class UserRole(str, enum.Enum):
    owner = "owner"
    admin = "admin"
    moderator = "moderator"
    user = "user"
    restricted = "restricted"
    banned = "banned"


class User(Base):
    __tablename__ = "users"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
    username: Mapped[str | None] = mapped_column(String(64))
    first_name: Mapped[str | None] = mapped_column(String(128))
    role: Mapped[UserRole] = mapped_column(Enum(UserRole), default=UserRole.user, nullable=False)
    language: Mapped[str] = mapped_column(String(8), default="en", nullable=False)
    theme: Mapped[str] = mapped_column(String(32), default="macchiato", nullable=False)
    ban_until: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    photo_url: Mapped[str | None] = mapped_column(String(512))
    is_premium: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=_now, nullable=False)
    updated_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=_now, onupdate=_now, nullable=False)

    group_policies: Mapped[list[UserGroupPolicy]] = relationship(
        back_populates="user", cascade="all, delete-orphan"
    )

    @property
    def is_blocked(self) -> bool:
        from datetime import timezone
        now = datetime.now(timezone.utc)
        if self.role == UserRole.banned:
            return True
        if self.ban_until is not None:
            bu = self.ban_until
            if bu.tzinfo is None:
                bu = bu.replace(tzinfo=timezone.utc)
            return bu > now
        return False


class Group(Base):
    __tablename__ = "groups"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True)
    title: Mapped[str | None] = mapped_column(String(256))
    enabled: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    auto_grant_role: Mapped[UserRole] = mapped_column(
        Enum(UserRole), default=UserRole.user, nullable=False
    )
    allow_pm: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)
    nsfw_allowed: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    storage_chat_id: Mapped[int | None] = mapped_column(BigInteger)
    storage_thread_id: Mapped[int | None] = mapped_column(BigInteger)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=_now, nullable=False)

    thread_policies: Mapped[list[ThreadPolicy]] = relationship(
        back_populates="group", cascade="all, delete-orphan"
    )
    user_policies: Mapped[list[UserGroupPolicy]] = relationship(
        back_populates="group", cascade="all, delete-orphan"
    )


class ThreadPolicy(Base):
    __tablename__ = "thread_policies"
    __table_args__ = (UniqueConstraint("group_id", "thread_id"),)

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    group_id: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("groups.id", ondelete="CASCADE"), nullable=False
    )
    thread_id: Mapped[int | None] = mapped_column(BigInteger)
    name: Mapped[str | None] = mapped_column(String(256))
    enabled: Mapped[bool] = mapped_column(Boolean, default=True, nullable=False)

    group: Mapped[Group] = relationship(back_populates="thread_policies")


class UserGroupPolicy(Base):
    __tablename__ = "user_group_policies"
    __table_args__ = (UniqueConstraint("user_id", "group_id"),)

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    group_id: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("groups.id", ondelete="CASCADE"), nullable=False
    )
    role_override: Mapped[UserRole | None] = mapped_column(Enum(UserRole))
    allow_pm_override: Mapped[bool | None] = mapped_column(Boolean)

    user: Mapped[User] = relationship(back_populates="group_policies")
    group: Mapped[Group] = relationship(back_populates="user_policies")


class BotSetting(Base):
    """Global admin-editable key-value settings."""
    __tablename__ = "bot_settings"

    key: Mapped[str] = mapped_column(String(64), primary_key=True)
    value: Mapped[str | None] = mapped_column(Text)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )


class Event(Base):
    """Append-only cross-plugin analytics event log."""
    __tablename__ = "events"
    __table_args__ = (
        Index("idx_events_user_created", "user_id", "created_at"),
        Index("idx_events_type_created", "event_type", "created_at"),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    plugin: Mapped[str] = mapped_column(String(32), nullable=False)
    event_type: Mapped[str] = mapped_column(String(64), nullable=False)
    user_id: Mapped[int | None] = mapped_column(BigInteger)
    group_id: Mapped[int | None] = mapped_column(BigInteger)
    thread_id: Mapped[int | None] = mapped_column(BigInteger)
    payload: Mapped[dict | None] = mapped_column(JSON)
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )


class UserPermission(Base):
    """Per-user feature access grant.

    Replaces per-plugin allowlist tables (e.g. insight_access).
    plugin + feature form a compound natural key alongside user_id.
    expires_at=None means the grant never expires.
    """
    __tablename__ = "user_permissions"
    __table_args__ = (
        UniqueConstraint("user_id", "plugin", "feature"),
        Index("idx_user_permissions_user", "user_id"),
    )

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger, ForeignKey("users.id", ondelete="CASCADE"), nullable=False
    )
    plugin: Mapped[str] = mapped_column(String(32), nullable=False)
    feature: Mapped[str] = mapped_column(String(64), nullable=False)
    granted_by: Mapped[int] = mapped_column(BigInteger, nullable=False)
    granted_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True), nullable=True)
    # Source of the grant: "manual" (admin action) or "tag" (automatic via tag_map)
    grant_source: Mapped[str] = mapped_column(String(16), nullable=False, server_default="manual")


class ApiKey(Base):
    """Long-lived API keys for M2M (machine-to-machine) access."""
    __tablename__ = "api_keys"

    id: Mapped[int] = mapped_column(Integer, primary_key=True, autoincrement=True)
    name: Mapped[str] = mapped_column(String(128), nullable=False)
    key_hash: Mapped[str] = mapped_column(String(64), unique=True, nullable=False)
    prefix: Mapped[str] = mapped_column(String(12), nullable=False)
    scopes: Mapped[list] = mapped_column(JSON, nullable=False, default=list)
    created_by: Mapped[int] = mapped_column(BigInteger, nullable=False)
    last_used_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    expires_at: Mapped[datetime | None] = mapped_column(DateTime(timezone=True))
    revoked: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    created_at: Mapped[datetime] = mapped_column(DateTime(timezone=True), default=_now, nullable=False)
