"""cookie pool: add content_hash for deduplication

Revision ID: 0024_cookie_content_hash
Revises: 0023_cookie_avatar_url
Create Date: 2026-03-29
"""
from alembic import op
import sqlalchemy as sa

revision = "0024_cookie_content_hash"
down_revision = "0023_cookie_avatar_url"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("cookies", sa.Column("content_hash", sa.String(64), nullable=True))
    op.create_index("ix_cookies_content_hash", "cookies", ["content_hash"])


def downgrade() -> None:
    op.drop_index("ix_cookies_content_hash", "cookies")
    op.drop_column("cookies", "content_hash")
