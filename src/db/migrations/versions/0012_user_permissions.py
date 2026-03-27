"""user_permissions table + migrate insight_access data

Revision ID: 0012_user_permissions
Revises: 0011_gallery_zip
Create Date: 2026-03-27

Creates the unified user_permissions table and migrates all existing
insight_access rows into it (plugin="insight", feature="summary").
The insight_access table is kept as a compatibility view/alias so the
old InsightAccess ORM model still works during the transition period.
"""
from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "0012_user_permissions"
down_revision: Union[str, None] = "0011_gallery_zip"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "user_permissions",
        sa.Column("id", sa.Integer(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("plugin", sa.String(32), nullable=False),
        sa.Column("feature", sa.String(64), nullable=False),
        sa.Column("granted_by", sa.BigInteger(), nullable=False),
        sa.Column("granted_at", sa.DateTime(timezone=True), nullable=False,
                  server_default=sa.func.now()),
        sa.Column("expires_at", sa.DateTime(timezone=True), nullable=True),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "plugin", "feature"),
    )
    op.create_index("idx_user_permissions_user", "user_permissions", ["user_id"])

    # Migrate insight_access rows -> user_permissions
    conn = op.get_bind()
    rows = conn.execute(
        sa.text("SELECT user_id, granted_by, granted_at FROM insight_access")
    ).fetchall()
    if rows:
        conn.execute(
            sa.text(
                "INSERT INTO user_permissions (user_id, plugin, feature, granted_by, granted_at) "
                "VALUES (:uid, 'insight', 'summary', :gby, :gat) "
                "ON CONFLICT (user_id, plugin, feature) DO NOTHING"
            ),
            [{"uid": r.user_id, "gby": r.granted_by, "gat": r.granted_at} for r in rows],
        )


def downgrade() -> None:
    op.drop_index("idx_user_permissions_user", table_name="user_permissions")
    op.drop_table("user_permissions")
