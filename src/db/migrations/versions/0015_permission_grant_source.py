"""Add grant_source column to user_permissions.

Revision ID: 0015_permission_grant_source
Revises: 0014_stats_sender_tag
Create Date: 2026-03-27
"""
from alembic import op
import sqlalchemy as sa

revision = "0015_permission_grant_source"
down_revision = "0014_stats_sender_tag"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "user_permissions",
        sa.Column(
            "grant_source",
            sa.String(16),
            nullable=False,
            server_default="manual",
        ),
    )


def downgrade() -> None:
    op.drop_column("user_permissions", "grant_source")
