"""insight_usage_log: add route column (gateway|byok)

Revision ID: 0046_insight_usage_log_route
Revises: 0045_insight_user_byok
Create Date: 2026-05-27

Adds:
  - route VARCHAR(16) NOT NULL DEFAULT 'gateway'

Marks which path a /tldr (and future /summary, /about) invocation took:
'gateway' for requests routed through the local gateway, 'byok' for
requests routed directly through a user-owned LLM provider. Legacy rows
default to 'gateway' since BYOK was added after this log table existed
and pre-migration data is gateway-only by construction.
"""
import sqlalchemy as sa
from alembic import op

revision = "0046_insight_usage_log_route"
down_revision = "0045_insight_user_byok"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "insight_usage_log",
        sa.Column("route", sa.String(16), nullable=False, server_default="gateway"),
    )


def downgrade() -> None:
    op.drop_column("insight_usage_log", "route")
