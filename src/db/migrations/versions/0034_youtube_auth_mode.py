"""user settings: add youtube_auth_mode

Revision ID: 0034_youtube_auth_mode
Revises: 0033_insight_cache
Create Date: 2026-05-19
"""
import sqlalchemy as sa
from alembic import op

revision = "0034_youtube_auth_mode"
down_revision = "0033_insight_cache"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "dl_user_settings",
        sa.Column(
            "youtube_auth_mode",
            sa.String(16),
            nullable=False,
            server_default="cookies",
        ),
    )


def downgrade() -> None:
    op.drop_column("dl_user_settings", "youtube_auth_mode")
