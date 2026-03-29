"""Add group_members table.

Revision ID: 0029
Revises: 0028
"""
from alembic import op
import sqlalchemy as sa

revision = "0029_group_members"
down_revision = "0028_stats_reactions"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "stats_group_members",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column("chat_id", sa.BigInteger(), nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("in_chat", sa.Boolean(), nullable=False, default=True),
        sa.Column("status", sa.String(32), nullable=True),
        sa.Column("joined_date", sa.DateTime(timezone=True), nullable=True),
        sa.Column("synced_at", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("idx_sgm_chat_user", "stats_group_members", ["chat_id", "user_id"])
    op.create_unique_constraint("uq_sgm_chat_user", "stats_group_members", ["chat_id", "user_id"])


def downgrade() -> None:
    op.drop_table("stats_group_members")
