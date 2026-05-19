"""insight_tldr_aliases: user-defined /tldr prompt aliases

Revision ID: 0039_insight_tldr_aliases
Revises: 0038_dl_settings_audio_codec
Create Date: 2026-05-19
"""
import sqlalchemy as sa
from alembic import op

revision = "0039_insight_tldr_aliases"
down_revision = "0038_dl_settings_audio_codec"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "insight_tldr_aliases",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("alias", sa.String(32), nullable=False),
        sa.Column("prompt", sa.Text(), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("idx_insight_tldr_alias_user", "insight_tldr_aliases", ["user_id"])


def downgrade() -> None:
    op.drop_index("idx_insight_tldr_alias_user", table_name="insight_tldr_aliases")
    op.drop_table("insight_tldr_aliases")
