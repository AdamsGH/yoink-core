"""insight_user_settings: per-user use_search toggle

Revision ID: 0044_insight_use_search
Revises: 0043_insight_user_prompts
Create Date: 2026-05-26

When True AND the user holds the 'insight:search' feature, fetch goes through
the gateway /v1/search (mode=raw) instead of the local trafilatura/jina stack.
"""
import sqlalchemy as sa
from alembic import op

revision = "0044_insight_use_search"
down_revision = "0043_insight_user_prompts"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_user_settings",
        sa.Column("use_search", sa.Boolean(), nullable=False, server_default=sa.false()),
    )


def downgrade() -> None:
    op.drop_column("insight_user_settings", "use_search")
