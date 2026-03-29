"""Add stats_chat_admins table.

Revision ID: 0030
Revises: 0029
"""
from alembic import op
import sqlalchemy as sa

revision = "0030_chat_admins"
down_revision = "0029_group_members"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "stats_chat_admins",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("chat_id", sa.BigInteger(), nullable=False),
        sa.Column("status", sa.String(16), nullable=False),
        sa.Column("synced_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_primary_key("pk_stats_chat_admins", "stats_chat_admins", ["user_id", "chat_id"])
    op.create_index("idx_sca_chat_id", "stats_chat_admins", ["chat_id"])


def downgrade() -> None:
    op.drop_table("stats_chat_admins")
