"""initial schema

Revision ID: 0001_initial
Revises:
Create Date: 2026-03-19

"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0001_initial"
down_revision: Union[str, None] = None
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "users",
        sa.Column("id", sa.BigInteger(), nullable=False),
        sa.Column("username", sa.String(64), nullable=True),
        sa.Column("first_name", sa.String(128), nullable=True),
        sa.Column(
            "role",
            sa.Enum("owner", "admin", "moderator", "user", "restricted", "banned", name="userrole"),
            nullable=False,
            server_default="user",
        ),
        sa.Column("language", sa.String(8), nullable=False, server_default="en"),
        sa.Column("theme", sa.String(32), nullable=False, server_default="macchiato"),
        sa.Column("ban_until", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id"),
    )

    op.create_table(
        "groups",
        sa.Column("id", sa.BigInteger(), nullable=False),
        sa.Column("title", sa.String(256), nullable=True),
        sa.Column("enabled", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column(
            "auto_grant_role",
            sa.Enum("owner", "admin", "moderator", "user", "restricted", "banned", name="userrole"),
            nullable=False,
            server_default="user",
        ),
        sa.Column("allow_pm", sa.Boolean(), nullable=False, server_default="true"),
        sa.Column("nsfw_allowed", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id"),
    )

    op.create_table(
        "thread_policies",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("group_id", sa.BigInteger(), nullable=False),
        sa.Column("thread_id", sa.BigInteger(), nullable=True),
        sa.Column("name", sa.String(256), nullable=True),
        sa.Column("enabled", sa.Boolean(), nullable=False, server_default="true"),
        sa.ForeignKeyConstraint(["group_id"], ["groups.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("group_id", "thread_id"),
    )

    op.create_table(
        "user_group_policies",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("group_id", sa.BigInteger(), nullable=False),
        sa.Column(
            "role_override",
            sa.Enum("owner", "admin", "moderator", "user", "restricted", "banned", name="userrole"),
            nullable=True,
        ),
        sa.Column("allow_pm_override", sa.Boolean(), nullable=True),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(["group_id"], ["groups.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "group_id"),
    )

    op.create_table(
        "bot_settings",
        sa.Column("key", sa.String(64), nullable=False),
        sa.Column("value", sa.Text(), nullable=True),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("key"),
    )

    op.create_table(
        "events",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("plugin", sa.String(32), nullable=False),
        sa.Column("event_type", sa.String(64), nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=True),
        sa.Column("group_id", sa.BigInteger(), nullable=True),
        sa.Column("thread_id", sa.BigInteger(), nullable=True),
        sa.Column("payload", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("idx_events_user_created", "events", ["user_id", "created_at"])
    op.create_index("idx_events_type_created", "events", ["event_type", "created_at"])


def downgrade() -> None:
    op.drop_index("idx_events_type_created", table_name="events")
    op.drop_index("idx_events_user_created", table_name="events")
    op.drop_table("events")
    op.drop_table("bot_settings")
    op.drop_table("user_group_policies")
    op.drop_table("thread_policies")
    op.drop_table("groups")
    op.drop_table("users")
    op.execute("DROP TYPE IF EXISTS userrole")
