"""add file_count to download_log

Revision ID: 0016
Revises: 0015
Create Date: 2026-03-27
"""
from alembic import op
import sqlalchemy as sa

revision = '0016'
down_revision = '0015'
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column('download_log', sa.Column('file_count', sa.Integer(), nullable=True))


def downgrade() -> None:
    op.drop_column('download_log', 'file_count')
