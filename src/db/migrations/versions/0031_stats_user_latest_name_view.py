"""Add stats_user_latest_name view.

Replaces repeated LATERAL subqueries against stats_user_names with a
simple LEFT JOIN. The view uses DISTINCT ON which is efficient with the
existing (user_id, date DESC) index on stats_user_names.

Not materialised on purpose: stats_user_names is write-hot (every group
message may upsert a row when the sender's display name changes), so a
materialised view would need REFRESH on every write or a scheduled
rebuild. The live DISTINCT ON over an index lookup is cheaper at our
scale than maintaining a materialised copy. Switch to MATERIALIZED VIEW
+ scheduled REFRESH if the chat-members endpoint ever becomes the
dominant query and stats_user_names writes are batched.

Revision ID: 0031
Revises: 0030
"""
from alembic import op

revision = "0031_stats_user_latest_name_view"
down_revision = "0030_chat_admins"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("""
        CREATE VIEW stats_user_latest_name AS
        SELECT DISTINCT ON (user_id)
            user_id,
            username,
            display_name,
            date
        FROM stats_user_names
        ORDER BY user_id, date DESC
    """)


def downgrade() -> None:
    op.execute("DROP VIEW IF EXISTS stats_user_latest_name")
