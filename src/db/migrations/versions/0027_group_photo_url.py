"""Add photo_url to groups table.

Revision ID: 0027
Revises: 0026
"""
from alembic import op
import sqlalchemy as sa

revision = "0027_group_photo_url"
down_revision = "0026_user_settings_use_pool"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("groups", sa.Column("photo_url", sa.String(512), nullable=True))


def downgrade() -> None:
    op.drop_column("groups", "photo_url")
