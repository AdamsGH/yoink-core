"""Restore the stats_user_latest_name view when the database has drifted."""

from pathlib import Path

from alembic import op

from yoink.core.db.query import load_sql

revision = "0054_restore_stats_latest_view"
down_revision = "0053_inbox_item_tg_reply"
branch_labels = None
depends_on = None

_SQL_DIR = Path(__file__).parent.parent / "sql"


def upgrade() -> None:
    op.execute(load_sql(_SQL_DIR, "0054_restore_stats_latest_view"))


def downgrade() -> None:
    # Revision 0031 owns the view, so reverting this repair must preserve it.
    pass
