"""cookie validated_at column

Revision ID: 0017
Revises: 0016
Create Date: 2026-03-27
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = '0017_cookie_validated_at'
down_revision = '0016_download_log_file_count'
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column('cookies', sa.Column('validated_at', sa.DateTime(timezone=True), nullable=True))


def downgrade() -> None:
    op.drop_column('cookies', 'validated_at')
