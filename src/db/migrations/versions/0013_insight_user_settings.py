"""Create insight_user_settings table; migrate lang from insight_access.

Revision ID: 0013
Revises: 0012
Create Date: 2026-03-27
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = "0013_insight_user_settings"
down_revision = "0012_user_permissions"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "insight_user_settings",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("lang", sa.String(8), nullable=False, server_default="en"),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )
    # Migrate existing lang preferences from insight_access
    op.execute("""
        INSERT INTO insight_user_settings (user_id, lang)
        SELECT user_id, lang FROM insight_access
        ON CONFLICT (user_id) DO NOTHING
    """)


def downgrade() -> None:
    op.drop_table("insight_user_settings")
