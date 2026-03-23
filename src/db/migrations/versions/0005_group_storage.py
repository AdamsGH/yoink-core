"""Add storage_chat_id and storage_thread_id to groups.

Revision ID: 0005_group_storage
Revises: 0004_stats_tsvector
Create Date: 2026-03-20
"""
from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0005_group_storage"
down_revision: Union[str, Sequence[str], None] = "0004_stats_tsvector"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column("groups", sa.Column("storage_chat_id", sa.BigInteger(), nullable=True))
    op.add_column("groups", sa.Column("storage_thread_id", sa.BigInteger(), nullable=True))


def downgrade() -> None:
    op.drop_column("groups", "storage_thread_id")
    op.drop_column("groups", "storage_chat_id")
