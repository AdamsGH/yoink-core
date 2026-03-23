"""stats plugin schema

Revision ID: 0003_stats_plugin
Revises: 0002_dl_plugin
Create Date: 2026-03-19

"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0003_stats_plugin"
down_revision: Union[str, None] = "0002_dl_plugin"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "stats_messages",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("message_id", sa.BigInteger(), nullable=False),
        sa.Column("chat_id", sa.BigInteger(), nullable=False),
        sa.Column("date", sa.DateTime(timezone=True), nullable=False),
        sa.Column("from_user", sa.BigInteger(), nullable=True),
        sa.Column("reply_to_message", sa.BigInteger(), nullable=True),
        sa.Column("forward_from", sa.BigInteger(), nullable=True),
        sa.Column("forward_from_chat", sa.BigInteger(), nullable=True),
        sa.Column("text", sa.Text(), nullable=True),
        sa.Column("caption", sa.Text(), nullable=True),
        sa.Column("msg_type", sa.String(32), nullable=False, server_default="text"),
        sa.Column("sticker_set_name", sa.String(128), nullable=True),
        sa.Column("new_chat_title", sa.String(256), nullable=True),
        sa.Column("file_id", sa.String(256), nullable=True),
        sa.Column("is_edited", sa.Boolean(), nullable=False, server_default="false"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("idx_stats_msg_chat_date", "stats_messages", ["chat_id", "date"])
    op.create_index("idx_stats_msg_user_date", "stats_messages", ["from_user", "date"])
    op.create_index("idx_stats_msg_type", "stats_messages", ["msg_type"])
    op.create_index("idx_stats_msg_chat", "stats_messages", ["chat_id"])

    op.create_table(
        "stats_user_events",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("message_id", sa.BigInteger(), nullable=False),
        sa.Column("chat_id", sa.BigInteger(), nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("date", sa.DateTime(timezone=True), nullable=False),
        sa.Column("event", sa.String(32), nullable=False),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("idx_stats_ue_chat_date", "stats_user_events", ["chat_id", "date"])

    op.create_table(
        "stats_user_names",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("date", sa.DateTime(timezone=True), nullable=False),
        sa.Column("username", sa.String(64), nullable=True),
        sa.Column("display_name", sa.String(256), nullable=True),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "date"),
    )
    op.create_index("idx_stats_un_user_date", "stats_user_names", ["user_id", "date"])


def downgrade() -> None:
    op.drop_index("idx_stats_un_user_date", table_name="stats_user_names")
    op.drop_table("stats_user_names")
    op.drop_index("idx_stats_ue_chat_date", table_name="stats_user_events")
    op.drop_table("stats_user_events")
    op.drop_index("idx_stats_msg_chat", table_name="stats_messages")
    op.drop_index("idx_stats_msg_type", table_name="stats_messages")
    op.drop_index("idx_stats_msg_user_date", table_name="stats_messages")
    op.drop_index("idx_stats_msg_chat_date", table_name="stats_messages")
    op.drop_table("stats_messages")
