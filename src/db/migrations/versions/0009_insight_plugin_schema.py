"""insight plugin schema

Revision ID: 0009_insight_plugin
Revises: 0008_api_keys
Create Date: 2026-03-27

"""
from __future__ import annotations

from typing import TYPE_CHECKING

import sqlalchemy as sa
from alembic import op

if TYPE_CHECKING:
    from collections.abc import Sequence

revision: str = "0009_insight_plugin"
down_revision: str | None = "0008_api_keys"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_table(
        "insight_access",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("lang", sa.String(8), nullable=False, server_default="en"),
        sa.Column("granted_by", sa.BigInteger(), nullable=False),
        sa.Column("granted_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )


def downgrade() -> None:
    op.drop_table("insight_access")
