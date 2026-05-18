"""Add is_premium column to users.

Revision ID: 0006_user_is_premium
Revises: 0005_group_storage
Create Date: 2026-03-21
"""
from collections.abc import Sequence

import sqlalchemy as sa
from alembic import op

revision: str = "0006_user_is_premium"
down_revision: str | Sequence[str] | None = "0005_group_storage"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.add_column(
        "users",
        sa.Column("is_premium", sa.Boolean(), nullable=False, server_default="false"),
    )


def downgrade() -> None:
    op.drop_column("users", "is_premium")
