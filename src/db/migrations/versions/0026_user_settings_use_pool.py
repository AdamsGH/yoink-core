"""user settings: add use_pool_cookies flag

Revision ID: 0026_user_settings_use_pool
Revises: 0025_cookie_session_key
Create Date: 2026-03-29
"""
from alembic import op
import sqlalchemy as sa

revision = "0026_user_settings_use_pool"
down_revision = "0025_cookie_session_key"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "dl_user_settings",
        sa.Column("use_pool_cookies", sa.Boolean(), nullable=False, server_default="true"),
    )


def downgrade() -> None:
    op.drop_column("dl_user_settings", "use_pool_cookies")
