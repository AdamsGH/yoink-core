"""Add sender_tag column to stats_messages.

Revision ID: 0014_stats_sender_tag
Revises: 0013_insight_user_settings
Create Date: 2026-03-27
"""
from alembic import op
import sqlalchemy as sa

revision = "0014_stats_sender_tag"
down_revision = "0013_insight_user_settings"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "stats_messages",
        sa.Column("sender_tag", sa.String(64), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("stats_messages", "sender_tag")
