"""insight_usage_log: add tldr metric columns (chars / video seconds / alias key)

Revision ID: 0042_tldr_usage_metrics
Revises: 0041_tldr_alias_domains
Create Date: 2026-05-26

Adds:
  - content_chars INT          NULL  -- characters of extracted content sent to LLM
  - video_seconds INT          NULL  -- video duration in seconds (YouTube only)
  - alias_key     VARCHAR(64)  NULL  -- resolved alias name when one was applied
"""
import sqlalchemy as sa
from alembic import op

revision = "0042_tldr_usage_metrics"
down_revision = "0041_tldr_alias_domains"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("insight_usage_log", sa.Column("content_chars", sa.Integer(), nullable=True))
    op.add_column("insight_usage_log", sa.Column("video_seconds", sa.Integer(), nullable=True))
    op.add_column("insight_usage_log", sa.Column("alias_key", sa.String(64), nullable=True))


def downgrade() -> None:
    op.drop_column("insight_usage_log", "alias_key")
    op.drop_column("insight_usage_log", "video_seconds")
    op.drop_column("insight_usage_log", "content_chars")
