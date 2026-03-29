"""Extend file_cache.cache_key from 64 to 80 chars to fit group cache keys (sha256:index).

Revision ID: 0019_file_cache_key_length
Revises: 0018_user_photo_url
Create Date: 2026-03-29
"""
from __future__ import annotations

from alembic import op
import sqlalchemy as sa

revision = '0019_file_cache_key_length'
down_revision = '0018_user_photo_url'
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.alter_column(
        'file_cache', 'cache_key',
        existing_type=sa.String(64),
        type_=sa.String(80),
        existing_nullable=False,
    )


def downgrade() -> None:
    op.alter_column(
        'file_cache', 'cache_key',
        existing_type=sa.String(80),
        type_=sa.String(64),
        existing_nullable=False,
    )
