"""insight_user_settings: add github_token column

Revision ID: 0037_insight_user_settings_github_token
Revises: 0036_insight_settings_tldr_model
Create Date: 2026-05-19
"""
import sqlalchemy as sa
from alembic import op

revision = "0037_user_settings_github_token"
down_revision = "0036_insight_settings_tldr_model"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_user_settings",
        sa.Column("github_token", sa.String(256), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("insight_user_settings", "github_token")
