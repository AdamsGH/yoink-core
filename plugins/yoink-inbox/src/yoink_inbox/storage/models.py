"""Inbox plugin ORM models.

Schema covers phase 1+2 of TODO-c66b1c02:

- inbox_teams / inbox_team_members  team abstraction for category sharing
                                    (independent from Telegram chat groups)
- inbox_items                       links / repos / articles dropped into the inbox
- inbox_categories                  per-user, optionally team-shared taxonomy
- inbox_item_categories             M:M item -> category with attached_by audit
- inbox_gh_stars                    snapshot of a user's GitHub starred repos
- inbox_gh_folders                  user-defined folders over starred repos
- inbox_gh_folder_members           M:M star -> folder with added_by audit
- inbox_item_to_star                bridge: inbox item identified as a GH repo
- inbox_rules                       automation rules (trigger / conditions / actions)

Naming convention (matches insight): table names lowercase snake_case, FKs to
users.id are BigInteger ON DELETE CASCADE, timestamps via _now() default. No
SQLA annotation imports under TYPE_CHECKING - SQLA eval()s annotations at
class construction time (kb:yoink:sqla-mapped-runtime-eval).
"""
from __future__ import annotations

from datetime import datetime  # noqa: TC003

from sqlalchemy import (
    JSON,
    BigInteger,
    Boolean,
    Computed,
    DateTime,
    Float,
    ForeignKey,
    Index,
    Integer,
    String,
    Text,
    UniqueConstraint,
)
from sqlalchemy.dialects.postgresql import JSONB
from sqlalchemy.orm import Mapped, mapped_column

from yoink.core.db.base import Base, _now


# ---------------------------------------------------------------------------
# Teams (category sharing abstraction; deliberately independent of TG groups)
# ---------------------------------------------------------------------------


class InboxTeam(Base):
    """A team of users sharing a slice of the inbox.

    Owner can rename and delete; admins can manage members; members can ingest
    into categories shared with the team. Independent of `groups` (TG-chat).
    """
    __tablename__ = "inbox_teams"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    name: Mapped[str] = mapped_column(String(128), nullable=False)
    slug: Mapped[str] = mapped_column(String(64), nullable=False)
    description: Mapped[str | None] = mapped_column(Text, nullable=True)
    owner_user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        UniqueConstraint("owner_user_id", "slug", name="uq_inbox_teams_owner_slug"),
    )


class InboxTeamMember(Base):
    """Membership of a user in an inbox team."""
    __tablename__ = "inbox_team_members"

    team_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_teams.id", ondelete="CASCADE"),
        primary_key=True,
    )
    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        primary_key=True,
    )
    role: Mapped[str] = mapped_column(
        String(16), nullable=False, default="member",
    )  # owner | admin | member
    joined_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        Index("ix_inbox_team_members_user", "user_id"),
    )


# ---------------------------------------------------------------------------
# Items
# ---------------------------------------------------------------------------


class InboxItem(Base):
    """A single saved link / piece of content in the inbox.

    `normalized_url` is the canonical form (strip utm_*, fragment, lowercase
    host) and drives dedup within the dedup window from InboxConfig.
    """
    __tablename__ = "inbox_items"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
    )

    url: Mapped[str] = mapped_column(Text, nullable=False)
    normalized_url: Mapped[str] = mapped_column(Text, nullable=False)

    title: Mapped[str | None] = mapped_column(Text, nullable=True)
    summary: Mapped[str | None] = mapped_column(Text, nullable=True)

    kind: Mapped[str] = mapped_column(
        String(24), nullable=False, default="link",
    )  # link | github_repo | article | video | other
    source: Mapped[str] = mapped_column(
        String(16), nullable=False, default="bot",
    )  # bot | web | api | extension
    status: Mapped[str] = mapped_column(
        String(16), nullable=False, default="pending",
    )  # pending | enriched | classified | archived | failed
    crawl_status: Mapped[str | None] = mapped_column(String(16), nullable=True)
    llm_status: Mapped[str | None] = mapped_column(String(16), nullable=True)

    content_text: Mapped[str | None] = mapped_column(Text, nullable=True)
    content_html_asset_id: Mapped[str | None] = mapped_column(String(64), nullable=True)

    og_image_url: Mapped[str | None] = mapped_column(Text, nullable=True)
    favicon_url: Mapped[str | None] = mapped_column(Text, nullable=True)
    author: Mapped[str | None] = mapped_column(String(255), nullable=True)
    publisher: Mapped[str | None] = mapped_column(String(255), nullable=True)
    published_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    modified_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )
    archived_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    __table_args__ = (
        UniqueConstraint("user_id", "normalized_url", name="uq_inbox_items_user_url"),
        # Karakeep-style composite index for cursor pagination over filtered sets.
        Index(
            "ix_inbox_items_user_status_created",
            "user_id", "status", "created_at", "id",
        ),
        Index("ix_inbox_items_user_kind", "user_id", "kind"),
    )


# ---------------------------------------------------------------------------
# Categories
# ---------------------------------------------------------------------------


class InboxCategory(Base):
    """A category in the user's taxonomy.

    Owned by a single user; optionally shared with an inbox team. Mutations
    (rename / delete / reparent) are owner-only; team members may bind items.
    `normalized_name` is a generated column used for dedup across spellings.
    """
    __tablename__ = "inbox_categories"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    owner_user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
    )
    shared_with_team_id: Mapped[int | None] = mapped_column(
        BigInteger,
        ForeignKey("inbox_teams.id", ondelete="SET NULL"),
        nullable=True,
    )

    name: Mapped[str] = mapped_column(String(128), nullable=False)
    # Postgres generated column: lower + strip space/-/_ for dedup
    normalized_name: Mapped[str] = mapped_column(
        String(128),
        Computed(
            "lower(regexp_replace(name, '[ _-]', '', 'g'))",
            persisted=True,
        ),
        nullable=False,
    )
    slug: Mapped[str] = mapped_column(String(64), nullable=False)
    icon: Mapped[str | None] = mapped_column(String(32), nullable=True)
    color: Mapped[str | None] = mapped_column(String(16), nullable=True)
    kind: Mapped[str] = mapped_column(
        String(16), nullable=False, default="user",
    )  # ai | user | system
    parent_id: Mapped[int | None] = mapped_column(
        BigInteger,
        ForeignKey("inbox_categories.id", ondelete="SET NULL"),
        nullable=True,
    )
    description: Mapped[str | None] = mapped_column(Text, nullable=True)

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        UniqueConstraint(
            "owner_user_id", "normalized_name",
            name="uq_inbox_categories_owner_normalized",
        ),
        UniqueConstraint(
            "owner_user_id", "slug",
            name="uq_inbox_categories_owner_slug",
        ),
        Index(
            "ix_inbox_categories_shared_team",
            "shared_with_team_id",
            postgresql_where="shared_with_team_id IS NOT NULL",
        ),
    )


class InboxItemCategory(Base):
    """M:M binding of an item to a category, with audit trail."""
    __tablename__ = "inbox_item_categories"

    item_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_items.id", ondelete="CASCADE"),
        primary_key=True,
    )
    category_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_categories.id", ondelete="CASCADE"),
        primary_key=True,
    )
    attached_by: Mapped[str] = mapped_column(
        String(8), nullable=False,
    )  # ai | user | rule
    attached_by_user_id: Mapped[int | None] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="SET NULL"),
        nullable=True,
    )
    confidence: Mapped[float | None] = mapped_column(Float, nullable=True)
    attached_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        Index("ix_inbox_item_categories_category", "category_id"),
    )


# ---------------------------------------------------------------------------
# GitHub stars (the orbit-style viewer)
# ---------------------------------------------------------------------------


class InboxGhStar(Base):
    """Snapshot of a single starred repo for a single user.

    Re-syncing preserves `ai_labels` / `ai_summary` set by previous runs.
    `can_unstar` mirrors whether the user has an opted-in public_repo token at
    last sync time; UI uses it to decide whether to render the unstar button.
    """
    __tablename__ = "inbox_gh_stars"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
    )
    gh_repo_id: Mapped[int] = mapped_column(BigInteger, nullable=False)

    full_name: Mapped[str] = mapped_column(String(255), nullable=False)
    gh_node_id: Mapped[str | None] = mapped_column(String(64), nullable=True)
    owner_login: Mapped[str] = mapped_column(String(128), nullable=False)
    owner_avatar_url: Mapped[str | None] = mapped_column(Text, nullable=True)
    description: Mapped[str | None] = mapped_column(Text, nullable=True)
    language: Mapped[str | None] = mapped_column(String(64), nullable=True)
    topics: Mapped[list[str] | None] = mapped_column(JSONB, nullable=True)
    stargazers_count: Mapped[int] = mapped_column(Integer, nullable=False, default=0)
    html_url: Mapped[str] = mapped_column(Text, nullable=False)
    homepage: Mapped[str | None] = mapped_column(Text, nullable=True)
    archived: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)
    fork: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)

    starred_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    updated_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )

    ai_labels: Mapped[list[str] | None] = mapped_column(JSONB, nullable=True)
    ai_summary: Mapped[str | None] = mapped_column(Text, nullable=True)
    can_unstar: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)

    last_synced_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        UniqueConstraint("user_id", "gh_repo_id", name="uq_inbox_gh_stars_user_repo"),
        Index("ix_inbox_gh_stars_user_lang", "user_id", "language"),
        Index("ix_inbox_gh_stars_user_starred", "user_id", "starred_at"),
    )


class InboxGhFolder(Base):
    """User-defined folder over starred repos."""
    __tablename__ = "inbox_gh_folders"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
    )
    name: Mapped[str] = mapped_column(String(128), nullable=False)
    slug: Mapped[str] = mapped_column(String(64), nullable=False)
    description: Mapped[str | None] = mapped_column(Text, nullable=True)
    icon: Mapped[str | None] = mapped_column(String(32), nullable=True)
    parent_id: Mapped[int | None] = mapped_column(
        BigInteger,
        ForeignKey("inbox_gh_folders.id", ondelete="SET NULL"),
        nullable=True,
    )
    # GitHub List linkage (nullable - folders may have no GH List counterpart)
    gh_list_id: Mapped[str | None] = mapped_column(String(64), nullable=True)
    gh_list_slug: Mapped[str | None] = mapped_column(String(128), nullable=True)

    # UI ordering / pinning
    is_pinned: Mapped[bool] = mapped_column(Boolean, nullable=False, default=False)
    sort_order: Mapped[int] = mapped_column(Integer, nullable=False, default=0)

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        UniqueConstraint("user_id", "slug", name="uq_inbox_gh_folders_user_slug"),
        Index("ix_inbox_gh_folders_gh_list_id", "user_id", "gh_list_id"),
    )


class InboxGhFolderMember(Base):
    """M:M binding of a starred repo to a folder."""
    __tablename__ = "inbox_gh_folder_members"

    folder_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_gh_folders.id", ondelete="CASCADE"),
        primary_key=True,
    )
    gh_star_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_gh_stars.id", ondelete="CASCADE"),
        primary_key=True,
    )
    added_by: Mapped[str] = mapped_column(
        String(8), nullable=False,
    )  # ai | user | rule | sync
    added_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )

    __table_args__ = (
        Index("ix_inbox_gh_folder_members_star", "gh_star_id"),
    )


class InboxItemToStar(Base):
    """Bridge: an inbox item recognised as a GH repo links to its star row."""
    __tablename__ = "inbox_item_to_star"

    item_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_items.id", ondelete="CASCADE"),
        primary_key=True,
    )
    gh_star_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("inbox_gh_stars.id", ondelete="CASCADE"),
        primary_key=True,
    )
    folder_id: Mapped[int | None] = mapped_column(
        BigInteger,
        ForeignKey("inbox_gh_folders.id", ondelete="SET NULL"),
        nullable=True,
    )
    synced_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )


# ---------------------------------------------------------------------------
# Rules engine
# ---------------------------------------------------------------------------


class InboxRule(Base):
    """A user-defined automation rule.

    Triggers fire at the tail of classify and gh_sync. Conditions and actions
    are stored as JSONB for flexibility; matcher and action implementations
    live in services/rules.py. Lower `priority` wins; ties broken by id.
    """
    __tablename__ = "inbox_rules"

    id: Mapped[int] = mapped_column(BigInteger, primary_key=True, autoincrement=True)
    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
    )
    name: Mapped[str] = mapped_column(String(128), nullable=False)
    enabled: Mapped[bool] = mapped_column(Boolean, nullable=False, default=True)
    priority: Mapped[int] = mapped_column(Integer, nullable=False, default=100)

    trigger: Mapped[str] = mapped_column(
        String(24), nullable=False,
    )  # item_ingested | item_classified | star_synced
    conditions: Mapped[list[dict] | None] = mapped_column(JSON, nullable=True)
    actions: Mapped[list[dict] | None] = mapped_column(JSON, nullable=True)

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    modified_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )

    __table_args__ = (
        Index("ix_inbox_rules_user_enabled_priority", "user_id", "enabled", "priority"),
    )


# ---------------------------------------------------------------------------
# GitHub stars sync bookkeeping (one row per user)
# ---------------------------------------------------------------------------


class InboxGhSyncState(Base):
    """Per-user GitHub stars sync state.

    One row per user. `etag` is the value returned by GitHub on the previous
    successful /user/starred call; on the next call we send it back as
    `If-None-Match` and treat 304 as "nothing to do". `last_synced_at` gates
    the periodic refresh; the worker skips users whose sync is younger than
    the configured interval.

    Lives in inbox (not insight_user_settings) so the OAuth token table stays
    owned by the insight plugin and the sync bookkeeping stays owned by the
    plugin that uses it.
    """

    __tablename__ = "inbox_gh_sync_state"

    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        primary_key=True,
    )
    etag: Mapped[str | None] = mapped_column(Text, nullable=True)
    last_synced_at: Mapped[datetime | None] = mapped_column(
        DateTime(timezone=True), nullable=True
    )
    # "ok" | "not_modified" | "auth_failed" | "rate_limited" | "error"
    last_status: Mapped[str | None] = mapped_column(Text, nullable=True)
    last_error: Mapped[str | None] = mapped_column(Text, nullable=True)
    stars_count: Mapped[int] = mapped_column(Integer, default=0, nullable=False)

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )


class InboxUserSettings(Base):
    """Per-user inbox settings (classify hint, future prefs)."""

    __tablename__ = "inbox_user_settings"

    user_id: Mapped[int] = mapped_column(
        BigInteger,
        ForeignKey("users.id", ondelete="CASCADE"),
        primary_key=True,
    )
    # Free-text hint appended to the classification prompt (e.g. preferred
    # label language, domain hints, tone). Nullable = not set.
    classify_user_hint: Mapped[str | None] = mapped_column(Text, nullable=True)

    created_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, nullable=False
    )
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )


class InboxAdminSettings(Base):
    """Global admin-controlled settings stored as key/value rows.

    Keys in use:
      classify_system_prompt  -- overrides the base classification prompt
                                 built by classify._build_prompt
    """

    __tablename__ = "inbox_admin_settings"

    key: Mapped[str] = mapped_column(String(128), primary_key=True)
    value: Mapped[str | None] = mapped_column(Text, nullable=True)
    updated_at: Mapped[datetime] = mapped_column(
        DateTime(timezone=True), default=_now, onupdate=_now, nullable=False
    )
