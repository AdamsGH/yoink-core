"""insight_user_byok: per-user Bring-Your-Own-Key for /tldr

Revision ID: 0045_insight_user_byok
Revises: 0044_insight_use_search
Create Date: 2026-05-26

Users who don't have the insight:tldr feature can still use /tldr by bringing
their own OpenAI/Anthropic/Gemini/OpenRouter/Perplexity (or custom OpenAI/
Anthropic-compatible) endpoint and key. The admin globally toggles BYOK on
or off via bot_settings (key='insight_byok_enabled').
"""
import sqlalchemy as sa
from alembic import op

revision = "0045_insight_user_byok"
down_revision = "0044_insight_use_search"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "insight_user_byok",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("provider", sa.String(length=32), nullable=False),
        sa.Column("base_url", sa.String(length=512), nullable=True),
        sa.Column("api_key", sa.Text(), nullable=False),
        sa.Column("model", sa.String(length=128), nullable=False),
        sa.Column("models_json", sa.Text(), nullable=True),
        sa.Column("models_fetched_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("tested_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("test_error", sa.String(length=256), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )


def downgrade() -> None:
    op.drop_table("insight_user_byok")
