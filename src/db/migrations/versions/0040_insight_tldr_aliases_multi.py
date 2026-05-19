"""insight_tldr_aliases: rename alias -> aliases, widen to 256

Revision ID: 0040_insight_tldr_aliases_multi
Revises: 0039_insight_tldr_aliases
Create Date: 2026-05-19
"""
import sqlalchemy as sa
from alembic import op

revision = "0040_insight_tldr_aliases_multi"
down_revision = "0039_insight_tldr_aliases"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.alter_column(
        "insight_tldr_aliases",
        "alias",
        new_column_name="aliases",
        existing_type=sa.String(32),
        type_=sa.String(256),
        nullable=False,
    )


def downgrade() -> None:
    op.alter_column(
        "insight_tldr_aliases",
        "aliases",
        new_column_name="alias",
        existing_type=sa.String(256),
        type_=sa.String(32),
        nullable=False,
    )
