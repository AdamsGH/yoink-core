"""dl plugin schema

Revision ID: 0002_dl_plugin
Revises: 0001_initial
Create Date: 2026-03-19

"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0002_dl_plugin"
down_revision: Union[str, None] = "0001_initial"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "dl_user_settings",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("quality", sa.String(32), nullable=False, server_default="best"),
        sa.Column("codec", sa.String(16), nullable=False, server_default="avc1"),
        sa.Column("container", sa.String(8), nullable=False, server_default="mp4"),
        sa.Column("proxy_enabled", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("proxy_url", sa.String(512), nullable=True),
        sa.Column("keyboard", sa.String(8), nullable=False, server_default="2x3"),
        sa.Column("subs_enabled", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("subs_auto", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("subs_always_ask", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("subs_lang", sa.String(8), nullable=False, server_default="en"),
        sa.Column("split_size", sa.BigInteger(), nullable=False, server_default="2043000000"),
        sa.Column("nsfw_blur", sa.Boolean(), nullable=False, server_default="true"),
        sa.Column("mediainfo", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("send_as_file", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("args_json", sa.JSON(), nullable=False, server_default="{}"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )

    op.create_table(
        "download_log",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("url", sa.Text(), nullable=False),
        sa.Column("domain", sa.String(253), nullable=True),
        sa.Column("title", sa.Text(), nullable=True),
        sa.Column("quality", sa.String(32), nullable=True),
        sa.Column("file_size", sa.BigInteger(), nullable=True),
        sa.Column("duration", sa.Float(), nullable=True),
        sa.Column("status", sa.String(16), nullable=False, server_default="ok"),
        sa.Column("error_msg", sa.Text(), nullable=True),
        sa.Column("group_id", sa.BigInteger(), nullable=True),
        sa.Column("thread_id", sa.BigInteger(), nullable=True),
        sa.Column("message_id", sa.BigInteger(), nullable=True),
        sa.Column("clip_start", sa.Integer(), nullable=True),
        sa.Column("clip_end", sa.Integer(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("idx_download_log_user", "download_log", ["user_id", "created_at"])

    op.create_table(
        "file_cache",
        sa.Column("cache_key", sa.String(64), nullable=False),
        sa.Column("file_id", sa.String(256), nullable=False),
        sa.Column("file_type", sa.String(16), nullable=False),
        sa.Column("title", sa.Text(), nullable=True),
        sa.Column("duration", sa.Float(), nullable=True),
        sa.Column("width", sa.Integer(), nullable=True),
        sa.Column("height", sa.Integer(), nullable=True),
        sa.Column("file_size", sa.BigInteger(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("cache_key"),
    )
    op.create_index("idx_file_cache_expires", "file_cache", ["expires_at"])

    op.create_table(
        "rate_limits",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("window", sa.String(16), nullable=False),
        sa.Column("count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("reset_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("user_id", "window"),
    )

    op.create_table(
        "cookies",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("domain", sa.String(253), nullable=False),
        sa.Column("content", sa.Text(), nullable=False),
        sa.Column("is_valid", sa.Boolean(), nullable=False, server_default="true"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "domain"),
    )

    op.create_table(
        "nsfw_domains",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("domain", sa.String(253), nullable=False),
        sa.Column("note", sa.String(256), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("domain"),
    )

    op.create_table(
        "nsfw_keywords",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("keyword", sa.String(128), nullable=False),
        sa.Column("note", sa.String(256), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("keyword"),
    )


def downgrade() -> None:
    op.drop_table("nsfw_keywords")
    op.drop_table("nsfw_domains")
    op.drop_table("cookies")
    op.drop_table("rate_limits")
    op.drop_index("idx_file_cache_expires", table_name="file_cache")
    op.drop_table("file_cache")
    op.drop_index("idx_download_log_user", table_name="download_log")
    op.drop_table("download_log")
    op.drop_table("dl_user_settings")
