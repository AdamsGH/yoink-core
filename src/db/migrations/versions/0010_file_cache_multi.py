"""file_cache: index on file_id for reverse lookup

Revision ID: 0010_file_cache_multi
Revises: 0009_insight_plugin
Create Date: 2026-03-27

"""
from __future__ import annotations

from typing import TYPE_CHECKING

from alembic import op

if TYPE_CHECKING:
    from collections.abc import Sequence

revision: str = "0010_file_cache_multi"
down_revision: str | None = "0009_insight_plugin"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.create_index("idx_file_cache_file_id", "file_cache", ["file_id"])


def downgrade() -> None:
    op.drop_index("idx_file_cache_file_id", table_name="file_cache")
