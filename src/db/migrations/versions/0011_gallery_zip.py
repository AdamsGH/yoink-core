"""dl_user_settings: add gallery_zip column

Revision ID: 0011_gallery_zip
Revises: 0010_file_cache_multi
Create Date: 2026-03-27

"""
from __future__ import annotations

from typing import TYPE_CHECKING

import sqlalchemy as sa
from alembic import op

if TYPE_CHECKING:
    from collections.abc import Sequence

revision: str = "0011_gallery_zip"
down_revision: str | None = "0010_file_cache_multi"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.add_column(
        "dl_user_settings",
        sa.Column("gallery_zip", sa.Boolean(), nullable=False, server_default="false"),
    )


def downgrade() -> None:
    op.drop_column("dl_user_settings", "gallery_zip")
