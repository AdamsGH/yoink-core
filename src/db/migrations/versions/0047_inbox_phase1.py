"""inbox phase 1+2 schema

Revision ID: 0047_inbox_phase1
Revises: 0046_insight_usage_log_route
Create Date: 2026-06-12

Adds the full schema for the yoink-inbox plugin in one revision so rollback
is a single drop. Tables:

- inbox_teams / inbox_team_members  team abstraction for category sharing
- inbox_items                       saved links / repos / articles
- inbox_categories                  per-user taxonomy with optional team share
- inbox_item_categories             M:M binding with attached_by audit
- inbox_gh_stars                    per-user snapshot of GitHub stars
- inbox_gh_folders                  user-defined folders over stars
- inbox_gh_folder_members           M:M binding with added_by audit
- inbox_item_to_star                bridge inbox item -> star
- inbox_rules                       automation rules (trigger / conditions / actions)
"""
import sqlalchemy as sa
from alembic import op
from sqlalchemy.dialects import postgresql

revision = "0047_inbox_phase1"
down_revision = "0046_insight_usage_log_route"
branch_labels = None
depends_on = None


def upgrade() -> None:
    # ------------------------------------------------------------------
    # Teams
    # ------------------------------------------------------------------
    op.create_table(
        "inbox_teams",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("name", sa.String(length=128), nullable=False),
        sa.Column("slug", sa.String(length=64), nullable=False),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("owner_user_id", sa.BigInteger(), nullable=False),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["owner_user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("owner_user_id", "slug", name="uq_inbox_teams_owner_slug"),
    )
    op.create_index(
        "ix_inbox_teams_owner_user_id",
        "inbox_teams",
        ["owner_user_id"],
    )

    op.create_table(
        "inbox_team_members",
        sa.Column("team_id", sa.BigInteger(), nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("role", sa.String(length=16), nullable=False, server_default="member"),
        sa.Column("joined_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["team_id"], ["inbox_teams.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("team_id", "user_id"),
    )
    op.create_index(
        "ix_inbox_team_members_user",
        "inbox_team_members",
        ["user_id"],
    )

    # ------------------------------------------------------------------
    # Items
    # ------------------------------------------------------------------
    op.create_table(
        "inbox_items",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("url", sa.Text(), nullable=False),
        sa.Column("normalized_url", sa.Text(), nullable=False),
        sa.Column("title", sa.Text(), nullable=True),
        sa.Column("summary", sa.Text(), nullable=True),
        sa.Column("kind", sa.String(length=24), nullable=False, server_default="link"),
        sa.Column("source", sa.String(length=16), nullable=False, server_default="bot"),
        sa.Column("status", sa.String(length=16), nullable=False, server_default="pending"),
        sa.Column("crawl_status", sa.String(length=16), nullable=True),
        sa.Column("llm_status", sa.String(length=16), nullable=True),
        sa.Column("content_text", sa.Text(), nullable=True),
        sa.Column("content_html_asset_id", sa.String(length=64), nullable=True),
        sa.Column("og_image_url", sa.Text(), nullable=True),
        sa.Column("favicon_url", sa.Text(), nullable=True),
        sa.Column("author", sa.String(length=255), nullable=True),
        sa.Column("publisher", sa.String(length=255), nullable=True),
        sa.Column("published_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("modified_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("archived_at", sa.DateTime(timezone=True), nullable=True),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint("user_id", "normalized_url", name="uq_inbox_items_user_url"),
    )
    op.create_index(
        "ix_inbox_items_user_status_created",
        "inbox_items",
        ["user_id", "status", "created_at", "id"],
    )
    op.create_index(
        "ix_inbox_items_user_kind",
        "inbox_items",
        ["user_id", "kind"],
    )

    # ------------------------------------------------------------------
    # Categories
    # ------------------------------------------------------------------
    op.create_table(
        "inbox_categories",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("owner_user_id", sa.BigInteger(), nullable=False),
        sa.Column("shared_with_team_id", sa.BigInteger(), nullable=True),
        sa.Column("name", sa.String(length=128), nullable=False),
        sa.Column(
            "normalized_name",
            sa.String(length=128),
            sa.Computed(
                "lower(regexp_replace(name, '[ _-]', '', 'g'))",
                persisted=True,
            ),
            nullable=False,
        ),
        sa.Column("slug", sa.String(length=64), nullable=False),
        sa.Column("icon", sa.String(length=32), nullable=True),
        sa.Column("color", sa.String(length=16), nullable=True),
        sa.Column("kind", sa.String(length=16), nullable=False, server_default="user"),
        sa.Column("parent_id", sa.BigInteger(), nullable=True),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["owner_user_id"], ["users.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(
            ["shared_with_team_id"], ["inbox_teams.id"], ondelete="SET NULL"
        ),
        sa.ForeignKeyConstraint(
            ["parent_id"], ["inbox_categories.id"], ondelete="SET NULL"
        ),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "owner_user_id", "normalized_name",
            name="uq_inbox_categories_owner_normalized",
        ),
        sa.UniqueConstraint(
            "owner_user_id", "slug",
            name="uq_inbox_categories_owner_slug",
        ),
    )
    op.create_index(
        "ix_inbox_categories_shared_team",
        "inbox_categories",
        ["shared_with_team_id"],
        postgresql_where=sa.text("shared_with_team_id IS NOT NULL"),
    )

    op.create_table(
        "inbox_item_categories",
        sa.Column("item_id", sa.BigInteger(), nullable=False),
        sa.Column("category_id", sa.BigInteger(), nullable=False),
        sa.Column("attached_by", sa.String(length=8), nullable=False),
        sa.Column("attached_by_user_id", sa.BigInteger(), nullable=True),
        sa.Column("confidence", sa.Float(), nullable=True),
        sa.Column("attached_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["item_id"], ["inbox_items.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(
            ["category_id"], ["inbox_categories.id"], ondelete="CASCADE"
        ),
        sa.ForeignKeyConstraint(
            ["attached_by_user_id"], ["users.id"], ondelete="SET NULL"
        ),
        sa.PrimaryKeyConstraint("item_id", "category_id"),
    )
    op.create_index(
        "ix_inbox_item_categories_category",
        "inbox_item_categories",
        ["category_id"],
    )

    # ------------------------------------------------------------------
    # GitHub stars + folders
    # ------------------------------------------------------------------
    op.create_table(
        "inbox_gh_stars",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("gh_repo_id", sa.BigInteger(), nullable=False),
        sa.Column("full_name", sa.String(length=255), nullable=False),
        sa.Column("owner_login", sa.String(length=128), nullable=False),
        sa.Column("owner_avatar_url", sa.Text(), nullable=True),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("language", sa.String(length=64), nullable=True),
        sa.Column("topics", postgresql.JSONB(astext_type=sa.Text()), nullable=True),
        sa.Column("stargazers_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("html_url", sa.Text(), nullable=False),
        sa.Column("homepage", sa.Text(), nullable=True),
        sa.Column("archived", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("fork", sa.Boolean(), nullable=False, server_default=sa.false()),
        sa.Column("starred_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("updated_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column("ai_labels", postgresql.JSONB(astext_type=sa.Text()), nullable=True),
        sa.Column("ai_summary", sa.Text(), nullable=True),
        sa.Column(
            "can_unstar", sa.Boolean(), nullable=False, server_default=sa.false()
        ),
        sa.Column("last_synced_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "user_id", "gh_repo_id", name="uq_inbox_gh_stars_user_repo"
        ),
    )
    op.create_index(
        "ix_inbox_gh_stars_user_lang",
        "inbox_gh_stars",
        ["user_id", "language"],
    )
    op.create_index(
        "ix_inbox_gh_stars_user_starred",
        "inbox_gh_stars",
        ["user_id", "starred_at"],
    )

    op.create_table(
        "inbox_gh_folders",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("name", sa.String(length=128), nullable=False),
        sa.Column("slug", sa.String(length=64), nullable=False),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("icon", sa.String(length=32), nullable=True),
        sa.Column("parent_id", sa.BigInteger(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(
            ["parent_id"], ["inbox_gh_folders.id"], ondelete="SET NULL"
        ),
        sa.PrimaryKeyConstraint("id"),
        sa.UniqueConstraint(
            "user_id", "slug", name="uq_inbox_gh_folders_user_slug"
        ),
    )

    op.create_table(
        "inbox_gh_folder_members",
        sa.Column("folder_id", sa.BigInteger(), nullable=False),
        sa.Column("gh_star_id", sa.BigInteger(), nullable=False),
        sa.Column("added_by", sa.String(length=8), nullable=False),
        sa.Column("added_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(
            ["folder_id"], ["inbox_gh_folders.id"], ondelete="CASCADE"
        ),
        sa.ForeignKeyConstraint(
            ["gh_star_id"], ["inbox_gh_stars.id"], ondelete="CASCADE"
        ),
        sa.PrimaryKeyConstraint("folder_id", "gh_star_id"),
    )
    op.create_index(
        "ix_inbox_gh_folder_members_star",
        "inbox_gh_folder_members",
        ["gh_star_id"],
    )

    op.create_table(
        "inbox_item_to_star",
        sa.Column("item_id", sa.BigInteger(), nullable=False),
        sa.Column("gh_star_id", sa.BigInteger(), nullable=False),
        sa.Column("folder_id", sa.BigInteger(), nullable=True),
        sa.Column("synced_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["item_id"], ["inbox_items.id"], ondelete="CASCADE"),
        sa.ForeignKeyConstraint(
            ["gh_star_id"], ["inbox_gh_stars.id"], ondelete="CASCADE"
        ),
        sa.ForeignKeyConstraint(
            ["folder_id"], ["inbox_gh_folders.id"], ondelete="SET NULL"
        ),
        sa.PrimaryKeyConstraint("item_id", "gh_star_id"),
    )

    # ------------------------------------------------------------------
    # Rules
    # ------------------------------------------------------------------
    op.create_table(
        "inbox_rules",
        sa.Column("id", sa.BigInteger(), autoincrement=True, nullable=False),
        sa.Column("user_id", sa.BigInteger(), nullable=False),
        sa.Column("name", sa.String(length=128), nullable=False),
        sa.Column("enabled", sa.Boolean(), nullable=False, server_default=sa.true()),
        sa.Column("priority", sa.Integer(), nullable=False, server_default="100"),
        sa.Column("trigger", sa.String(length=24), nullable=False),
        sa.Column("conditions", sa.JSON(), nullable=True),
        sa.Column("actions", sa.JSON(), nullable=True),
        sa.Column("created_at", sa.DateTime(timezone=True), nullable=False),
        sa.Column("modified_at", sa.DateTime(timezone=True), nullable=False),
        sa.ForeignKeyConstraint(["user_id"], ["users.id"], ondelete="CASCADE"),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_inbox_rules_user_enabled_priority",
        "inbox_rules",
        ["user_id", "enabled", "priority"],
    )


def downgrade() -> None:
    op.drop_index("ix_inbox_rules_user_enabled_priority", table_name="inbox_rules")
    op.drop_table("inbox_rules")

    op.drop_table("inbox_item_to_star")

    op.drop_index("ix_inbox_gh_folder_members_star", table_name="inbox_gh_folder_members")
    op.drop_table("inbox_gh_folder_members")
    op.drop_table("inbox_gh_folders")

    op.drop_index("ix_inbox_gh_stars_user_starred", table_name="inbox_gh_stars")
    op.drop_index("ix_inbox_gh_stars_user_lang", table_name="inbox_gh_stars")
    op.drop_table("inbox_gh_stars")

    op.drop_index("ix_inbox_item_categories_category", table_name="inbox_item_categories")
    op.drop_table("inbox_item_categories")

    op.drop_index("ix_inbox_categories_shared_team", table_name="inbox_categories")
    op.drop_table("inbox_categories")

    op.drop_index("ix_inbox_items_user_kind", table_name="inbox_items")
    op.drop_index("ix_inbox_items_user_status_created", table_name="inbox_items")
    op.drop_table("inbox_items")

    op.drop_index("ix_inbox_team_members_user", table_name="inbox_team_members")
    op.drop_table("inbox_team_members")

    op.drop_index("ix_inbox_teams_owner_user_id", table_name="inbox_teams")
    op.drop_table("inbox_teams")
