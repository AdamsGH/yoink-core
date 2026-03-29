"""Add stats_reactions table.

Revision ID: 0028
Revises: 0027
"""
from alembic import op
import sqlalchemy as sa

revision = "0028_stats_reactions"
down_revision = "0027_group_photo_url"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "stats_reactions",
        sa.Column("id", sa.Integer(), primary_key=True, autoincrement=True),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("chat_id", sa.BigInteger(), nullable=False),
        sa.Column("message_id", sa.BigInteger(), nullable=False),
        sa.Column("reaction_key", sa.String(64), nullable=False),
        sa.Column("reaction_type", sa.String(16), nullable=False),
        sa.Column("date", sa.DateTime(timezone=True), nullable=False),
    )
    op.create_index("idx_stats_reactions_user_chat", "stats_reactions", ["user_id", "chat_id"])
    op.create_index("idx_stats_reactions_chat_date", "stats_reactions", ["chat_id", "date"])
    op.create_unique_constraint(
        "uq_stats_reactions_user_msg_key",
        "stats_reactions",
        ["user_id", "chat_id", "message_id", "reaction_key"],
    )


def downgrade() -> None:
    op.drop_table("stats_reactions")
