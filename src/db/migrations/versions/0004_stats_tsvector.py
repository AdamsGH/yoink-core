"""stats: add tsvector column and GIN index for full-text search

Revision ID: 0004_stats_tsvector
Revises: 0003_stats_plugin
Create Date: 2026-03-20

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects.postgresql import TSVECTOR

revision: str = "0004_stats_tsvector"
down_revision: Union[str, None] = "0003_stats_plugin"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "stats_messages",
        sa.Column("text_search", TSVECTOR, nullable=True),
    )
    op.create_index(
        "idx_stats_msg_fts",
        "stats_messages",
        ["text_search"],
        postgresql_using="gin",
    )
    op.execute("""
        CREATE OR REPLACE FUNCTION stats_messages_tsvector_update() RETURNS trigger AS $$
        BEGIN
            NEW.text_search := to_tsvector(
                'simple',
                COALESCE(NEW.text, '') || ' ' || COALESCE(NEW.caption, '')
            );
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;
    """)
    op.execute("""
        CREATE TRIGGER stats_messages_tsvector_trigger
        BEFORE INSERT OR UPDATE ON stats_messages
        FOR EACH ROW EXECUTE FUNCTION stats_messages_tsvector_update();
    """)
    op.execute("""
        UPDATE stats_messages
        SET text_search = to_tsvector(
            'simple',
            COALESCE(text, '') || ' ' || COALESCE(caption, '')
        )
        WHERE text IS NOT NULL OR caption IS NOT NULL;
    """)


def downgrade() -> None:
    op.execute("DROP TRIGGER IF EXISTS stats_messages_tsvector_trigger ON stats_messages;")
    op.execute("DROP FUNCTION IF EXISTS stats_messages_tsvector_update();")
    op.drop_index("idx_stats_msg_fts", table_name="stats_messages")
    op.drop_column("stats_messages", "text_search")
