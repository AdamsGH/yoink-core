"""Rename insight_summary_cache.video_id -> content_key (supports non-YouTube URLs).

Revision ID: 0035_insight_cache_content_key
Revises: 0034_youtube_auth_mode
Create Date: 2026-05-19
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "0035_insight_cache_content_key"
down_revision = "0034_youtube_auth_mode"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # Drop unique constraint that references the old column name
    op.drop_constraint("uq_insight_cache_key", "insight_summary_cache", type_="unique")

    # Rename video_id -> content_key and widen to 512 chars (URL can be long)
    op.alter_column(
        "insight_summary_cache",
        "video_id",
        new_column_name="content_key",
        type_=sa.String(512),
        existing_type=sa.String(32),
        existing_nullable=False,
    )

    # Re-create unique constraint under the new column name
    op.create_unique_constraint(
        "uq_insight_cache_key",
        "insight_summary_cache",
        ["content_key", "lang", "command"],
    )


def downgrade() -> None:
    op.drop_constraint("uq_insight_cache_key", "insight_summary_cache", type_="unique")
    op.alter_column(
        "insight_summary_cache",
        "content_key",
        new_column_name="video_id",
        type_=sa.String(32),
        existing_type=sa.String(512),
        existing_nullable=False,
    )
    op.create_unique_constraint(
        "uq_insight_cache_key",
        "insight_summary_cache",
        ["video_id", "lang", "command"],
    )
