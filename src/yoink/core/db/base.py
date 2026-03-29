"""Declarative base and shared mixins."""
from __future__ import annotations

from datetime import datetime, timezone

from sqlalchemy import DateTime
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column


def utcnow() -> datetime:
    """Current UTC datetime, suitable as SQLAlchemy column default."""
    return datetime.now(timezone.utc)


# Internal alias used by mapped_column(default=...) calls
_now = utcnow


class Base(DeclarativeBase):
    pass


class AuditMixin:
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )


class SoftDeleteMixin:
    deleted_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), default=None, nullable=True
    )
