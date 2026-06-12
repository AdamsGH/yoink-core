from alembic import op
import sqlalchemy as sa

revision = "0051_gh_folder_pin_order"
down_revision = "0050_inbox_gh_list_sync"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "inbox_gh_folders",
        sa.Column("is_pinned", sa.Boolean(), nullable=False, server_default="false"),
    )
    op.add_column(
        "inbox_gh_folders",
        sa.Column("sort_order", sa.Integer(), nullable=False, server_default="0"),
    )


def downgrade() -> None:
    op.drop_column("inbox_gh_folders", "sort_order")
    op.drop_column("inbox_gh_folders", "is_pinned")
