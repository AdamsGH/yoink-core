"""Add tldr_model column to insight_user_settings.

Revision ID: 0036_insight_user_settings_tldr_model
Revises: 0035_insight_cache_content_key
Create Date: 2026-05-19
"""
from __future__ import annotations

import sqlalchemy as sa
from alembic import op

revision = "0036_insight_settings_tldr_model"
down_revision = "0035_insight_cache_content_key"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_user_settings",
        sa.Column("tldr_model", sa.String(128), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("insight_user_settings", "tldr_model")
