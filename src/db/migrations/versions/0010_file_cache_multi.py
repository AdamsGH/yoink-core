"""file_cache: index on file_id for reverse lookup

Revision ID: 0010_file_cache_multi
Revises: 0009_insight_plugin
Create Date: 2026-03-27

"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0010_file_cache_multi"
down_revision: Union[str, None] = "0009_insight_plugin"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_index("idx_file_cache_file_id", "file_cache", ["file_id"])


def downgrade() -> None:
    op.drop_index("idx_file_cache_file_id", table_name="file_cache")
