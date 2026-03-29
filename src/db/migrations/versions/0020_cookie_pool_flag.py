"""cookie pool flag

Revision ID: 0020
Revises: 0019
Create Date: 2026-03-29
"""
from alembic import op
import sqlalchemy as sa

revision = "0020_cookie_pool_flag"
down_revision = "0019_file_cache_key_length"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "cookies",
        sa.Column("is_pool", sa.Boolean(), nullable=False, server_default="false"),
    )
    op.create_index("ix_cookies_is_pool_domain", "cookies", ["is_pool", "domain"])


def downgrade() -> None:
    op.drop_index("ix_cookies_is_pool_domain", table_name="cookies")
    op.drop_column("cookies", "is_pool")
