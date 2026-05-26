"""insight_user_prompts: per-user default prompt overrides per command

Revision ID: 0043_insight_user_prompts
Revises: 0042_tldr_usage_metrics
Create Date: 2026-05-26

Stores per-user prompt overrides for /summary, /about, and /tldr (default,
no-alias path). NULL or absent row means "use the built-in default".
"""
import sqlalchemy as sa
from alembic import op

revision = "0043_insight_user_prompts"
down_revision = "0042_tldr_usage_metrics"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "insight_user_prompts",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("command", sa.String(16), nullable=False),
        sa.Column("prompt", sa.Text(), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), server_default=sa.func.now(), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id", "command"),
    )


def downgrade() -> None:
    op.drop_table("insight_user_prompts")
