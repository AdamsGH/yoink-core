"""Pydantic schemas for the inbox REST API.

Kept in one file: 8 schemas total, not enough mass to justify splitting.
All field names mirror the ORM columns 1:1 so the response shape is
self-documenting against the DB.
"""
from __future__ import annotations

from datetime import datetime

from pydantic import AnyHttpUrl, BaseModel, ConfigDict, Field


class _Base(BaseModel):
    model_config = ConfigDict(from_attributes=True)


# ---------------------------------------------------------------------------
# Categories
# ---------------------------------------------------------------------------


class InboxCategoryRead(_Base):
    id: int
    name: str
    slug: str
    icon: str | None = None
    color: str | None = None
    kind: str
    parent_id: int | None = None
    description: str | None = None
    owner_user_id: int
    shared_with_team_id: int | None = None
    item_count: int = 0


# ---------------------------------------------------------------------------
# Items
# ---------------------------------------------------------------------------


class InboxItemCategoryRef(_Base):
    """Slim view of a category as bound to an item."""
    id: int
    name: str
    slug: str
    color: str | None = None
    attached_by: str
    confidence: float | None = None


class InboxItemRead(_Base):
    id: int
    url: str
    normalized_url: str
    title: str | None = None
    summary: str | None = None
    kind: str
    source: str
    status: str
    crawl_status: str | None = None
    llm_status: str | None = None
    og_image_url: str | None = None
    favicon_url: str | None = None
    author: str | None = None
    publisher: str | None = None
    published_at: datetime | None = None
    created_at: datetime
    modified_at: datetime
    archived_at: datetime | None = None
    categories: list[InboxItemCategoryRef] = Field(default_factory=list)


class InboxItemListResponse(BaseModel):
    items: list[InboxItemRead]
    next_cursor: str | None = None


class InboxItemCreate(BaseModel):
    url: AnyHttpUrl
    source: str = "api"  # api | web | extension


# ---------------------------------------------------------------------------
# GitHub stars
# ---------------------------------------------------------------------------


class InboxGhStarRead(_Base):
    id: int
    gh_repo_id: int
    full_name: str
    owner_login: str
    owner_avatar_url: str | None = None
    description: str | None = None
    language: str | None = None
    topics: list[str] | None = None
    stargazers_count: int
    html_url: str
    homepage: str | None = None
    archived: bool
    fork: bool
    starred_at: datetime | None = None
    updated_at: datetime | None = None
    ai_labels: list[str] | None = None
    ai_summary: str | None = None
    can_unstar: bool
    last_synced_at: datetime | None = None


class InboxGhStarListResponse(BaseModel):
    items: list[InboxGhStarRead]
    next_cursor: str | None = None
    sync_status: str | None = None
    last_synced_at: datetime | None = None
