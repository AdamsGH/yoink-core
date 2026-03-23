"""Add api_keys table for M2M authentication.

Revision ID: 0008_api_keys
Revises: 0007_dl_dm_topic
Create Date: 2026-03-23
"""
from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0008_api_keys"
down_revision: Union[str, Sequence[str], None] = "0007_dl_dm_topic"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    conn = op.get_bind()
    exists = conn.execute(sa.text(
        "SELECT 1 FROM information_schema.tables WHERE table_name = 'api_keys'"
    )).scalar()
    if exists:
        return

    op.create_table(
        "api_keys",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column("name", sa.String(128), nullable=False),
        sa.Column("key_hash", sa.String(64), unique=True, nullable=False),
        sa.Column("prefix", sa.String(12), nullable=False),
        sa.Column("scopes", sa.JSON(), nullable=False, server_default="[]"),
        sa.Column("created_by", sa.BigInteger(), nullable=False),
        sa.Column("last_used_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("revoked", sa.Boolean(), nullable=False, server_default="false"),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False, server_default=sa.func.now()),
    )


def downgrade() -> None:
    op.drop_table("api_keys")
