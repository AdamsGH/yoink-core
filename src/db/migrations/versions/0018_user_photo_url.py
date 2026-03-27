"""Add photo_url to users table.

Revision ID: 0018
Revises: 0017
"""
from alembic import op
import sqlalchemy as sa

revision = "0018_user_photo_url"
down_revision = "0017_cookie_validated_at"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("users", sa.Column("photo_url", sa.String(512), nullable=True))


def downgrade() -> None:
    op.drop_column("users", "photo_url")
