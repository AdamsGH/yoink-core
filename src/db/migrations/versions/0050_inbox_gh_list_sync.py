from alembic import op
import sqlalchemy as sa

revision = "0050_inbox_gh_list_sync"
down_revision = "0049_gh_write_token"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "inbox_gh_folders",
        sa.Column("gh_list_id", sa.String(64), nullable=True),
    )
    op.add_column(
        "inbox_gh_folders",
        sa.Column("gh_list_slug", sa.String(128), nullable=True),
    )
    op.create_index(
        "ix_inbox_gh_folders_gh_list_id",
        "inbox_gh_folders",
        ["user_id", "gh_list_id"],
    )
    op.add_column(
        "inbox_gh_stars",
        sa.Column("gh_node_id", sa.String(64), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("inbox_gh_stars", "gh_node_id")
    op.drop_index("ix_inbox_gh_folders_gh_list_id", table_name="inbox_gh_folders")
    op.drop_column("inbox_gh_folders", "gh_list_slug")
    op.drop_column("inbox_gh_folders", "gh_list_id")
