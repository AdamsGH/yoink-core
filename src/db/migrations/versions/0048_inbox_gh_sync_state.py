"""inbox: per-user GitHub stars sync state

Revision ID: 0048_inbox_gh_sync_state
Revises: 0047_inbox_phase1
Create Date: 2026-06-12

Adds one table:

- inbox_gh_sync_state: per-user record of the last successful /user/starred
  sync. Carries the ETag from the previous call so we can short-circuit on
  HTTP 304 without re-reading the full list. last_synced_at gates the
  periodic refresh.

Kept separate from insight_user_settings on purpose: token ownership stays
with yoink-insight (it owns the OAuth flow), but sync bookkeeping is inbox
plugin state and lives next to inbox_gh_stars.
"""
import sqlalchemy as sa
from alembic import op

revision = "0048_inbox_gh_sync_state"
down_revision = "0047_inbox_phase1"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "inbox_gh_sync_state",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("etag", sa.Text(), nullable=True),
        sa.Column("last_synced_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("last_status", sa.Text(), nullable=True),
        sa.Column("last_error", sa.Text(), nullable=True),
        sa.Column("stars_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.text("now()"),
        ),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            nullable=False,
            server_default=sa.text("now()"),
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )


def downgrade() -> None:
    op.drop_table("inbox_gh_sync_state")
