"""Add insight_summary_cache table.

Revision ID: 0033_insight_summary_cache
Revises: 0032_stats_messages_indexes_thread
Create Date: 2026-03-29
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "0033_insight_cache"
down_revision = "0032_stats_msg_idx"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "insight_summary_cache",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column("video_id", sa.String(32), nullable=False),
        sa.Column("lang", sa.String(8), nullable=False),
        sa.Column("command", sa.String(16), nullable=False),
        sa.Column("result", sa.Text(), nullable=False),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            server_default=sa.text("now()"),
            nullable=False,
        ),
        sa.Column(
            "expires_at",
            sa.DateTime(timezone=True),
            nullable=False,
        ),
        sa.UniqueConstraint("video_id", "lang", "command", name="uq_insight_cache_key"),
    )
    op.create_index(
        "idx_insight_cache_expires",
        "insight_summary_cache",
        ["expires_at"],
    )


def downgrade() -> None:
    op.drop_index("idx_insight_cache_expires", table_name="insight_summary_cache")
    op.drop_table("insight_summary_cache")
