from alembic import op
import sqlalchemy as sa

revision = "0053_inbox_item_tg_reply"
down_revision = "0052_inbox_prompt_settings"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column("inbox_items", sa.Column("tg_chat_id", sa.BigInteger(), nullable=True))
    op.add_column("inbox_items", sa.Column("tg_reply_message_id", sa.BigInteger(), nullable=True))


def downgrade() -> None:
    op.drop_column("inbox_items", "tg_reply_message_id")
    op.drop_column("inbox_items", "tg_chat_id")
