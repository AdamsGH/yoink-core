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
    # This model originally relied on runtime create_all(), so an empty database
    # could not reach revision 0042, which adds columns to this table. Keep its
    # baseline here with the plugin schema rather than synthesizing test tables.
    op.create_table(
        "insight_usage_log",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("command", sa.String(16), nullable=False),
        sa.Column("video_id", sa.String(32), nullable=True),
        sa.Column("lang", sa.String(8), nullable=False),
        sa.Column("status", sa.String(16), nullable=False),
        sa.Column("error_code", sa.String(32), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
    )
    op.create_index("idx_insight_usage_user_date", "insight_usage_log", ["user_id", "created_at"])


def downgrade() -> None:
    op.drop_index("idx_insight_usage_user_date", table_name="insight_usage_log")
    op.drop_table("insight_usage_log")
    op.drop_table("insight_access")
