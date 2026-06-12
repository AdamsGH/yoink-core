from alembic import op
import sqlalchemy as sa

revision = "0052_inbox_prompt_settings"
down_revision = "0051_gh_folder_pin_order"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.create_table(
        "inbox_user_settings",
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("classify_user_hint", sa.Text(), nullable=True),
        sa.Column(
            "created_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            nullable=False,
        ),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            onupdate=sa.func.now(),
            nullable=False,
        ),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("user_id"),
    )
    op.create_table(
        "inbox_admin_settings",
        sa.Column("key", sa.String(128), nullable=False),
        sa.Column("value", sa.Text(), nullable=True),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            onupdate=sa.func.now(),
            nullable=False,
        ),
        sa.PrimaryKeyConstraint("key"),
    )


def downgrade() -> None:
    op.drop_table("inbox_admin_settings")
    op.drop_table("inbox_user_settings")
