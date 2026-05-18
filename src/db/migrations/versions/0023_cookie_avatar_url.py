"""cookie pool: add avatar_url column

Revision ID: 0023_cookie_avatar_url
Revises: 0022_cookie_pool_partial_unique
Create Date: 2026-03-29
"""
import sqlalchemy as sa
from alembic import op

revision = "0023_cookie_avatar_url"
down_revision = "0022_cookie_pool_partial_unique"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("cookies", sa.Column("avatar_url", sa.String(512), nullable=True))


def downgrade() -> None:
    op.drop_column("cookies", "avatar_url")
