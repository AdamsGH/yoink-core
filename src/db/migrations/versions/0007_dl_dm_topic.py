"""Add dm_topic_thread_id to dl_user_settings.

Revision ID: 0007_dl_dm_topic
Revises: 0006_user_is_premium
Create Date: 2026-03-23
"""
from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0007_dl_dm_topic"
down_revision: Union[str, Sequence[str], None] = "0006_user_is_premium"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    conn = op.get_bind()
    exists = conn.execute(sa.text(
        "SELECT 1 FROM information_schema.columns "
        "WHERE table_name = 'dl_user_settings' AND column_name = 'dm_topic_thread_id'"
    )).scalar()
    if exists:
        return

    op.add_column(
        "dl_user_settings",
        sa.Column("dm_topic_thread_id", sa.BigInteger(), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("dl_user_settings", "dm_topic_thread_id")
