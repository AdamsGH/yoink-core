"""cookie pool: add session_key for stable per-account deduplication

Revision ID: 0025_cookie_session_key
Revises: 0024_cookie_content_hash
Create Date: 2026-03-29
"""
from alembic import op
import sqlalchemy as sa

revision = "0025_cookie_session_key"
down_revision = "0024_cookie_content_hash"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("cookies", sa.Column("session_key", sa.String(256), nullable=True))
    op.create_index("ix_cookies_session_key", "cookies", ["session_key"])


def downgrade() -> None:
    op.drop_index("ix_cookies_session_key", "cookies")
    op.drop_column("cookies", "session_key")
