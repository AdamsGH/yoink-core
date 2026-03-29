"""Add (chat_id, message_id) index to stats_messages; add message_thread_id column.

Revision ID: 0032_stats_messages_indexes_thread
Revises: 0031_stats_user_latest_name_view
Create Date: 2026-03-29
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "0032_stats_msg_idx"
down_revision = "0031_stats_user_latest_name_view"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "stats_messages",
        sa.Column("message_thread_id", sa.BigInteger(), nullable=True),
    )
    op.create_index(
        "idx_stats_msg_chat_msgid",
        "stats_messages",
        ["chat_id", "message_id"],
    )
    op.create_index(
        "idx_stats_msg_thread",
        "stats_messages",
        ["chat_id", "message_thread_id"],
        postgresql_where=sa.text("message_thread_id IS NOT NULL"),
    )


def downgrade() -> None:
    op.drop_index("idx_stats_msg_thread", table_name="stats_messages")
    op.drop_index("idx_stats_msg_chat_msgid", table_name="stats_messages")
    op.drop_column("stats_messages", "message_thread_id")
