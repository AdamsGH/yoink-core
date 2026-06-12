"""Pydantic schemas for the inbox REST API.

Kept in one file: 8 schemas total, not enough mass to justify splitting.
All field names mirror the ORM columns 1:1 so the response shape is
self-documenting against the DB.
"""
from __future__ import annotations

from datetime import datetime

import re

from pydantic import AnyHttpUrl, BaseModel, ConfigDict, Field, field_validator


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
# Categories (write)
# ---------------------------------------------------------------------------


def _derive_slug(name: str) -> str:
    return re.sub(r'[^a-z0-9]+', '-', name.lower()).strip('-')[:64]


class InboxCategoryCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    slug: str | None = None
    icon: str | None = None
    color: str | None = None
    description: str | None = None
    parent_id: int | None = None
    shared_with_team_id: int | None = None

    @field_validator('slug', mode='before')
    @classmethod
    def _auto_slug(cls, v: str | None, info) -> str:
        if v:
            return v[:64]
        # derive from name if present in validated data
        name = info.data.get('name', '')
        return _derive_slug(name) if name else ''


class InboxCategoryUpdate(InboxCategoryCreate):
    pass


# ---------------------------------------------------------------------------
# GH Folders
# ---------------------------------------------------------------------------


class InboxGhFolderRead(_Base):
    id: int
    user_id: int
    name: str
    slug: str
    description: str | None = None
    icon: str | None = None
    parent_id: int | None = None
    star_count: int = 0
    gh_list_id: str | None = None
    gh_list_slug: str | None = None
    created_at: datetime


class InboxGhFolderCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    slug: str | None = None
    description: str | None = None
    icon: str | None = None
    parent_id: int | None = None

    @field_validator('slug', mode='before')
    @classmethod
    def _auto_slug(cls, v: str | None, info) -> str:
        if v:
            return v[:64]
        name = info.data.get('name', '')
        return _derive_slug(name) if name else ''


# ---------------------------------------------------------------------------
# Teams
# ---------------------------------------------------------------------------


class InboxTeamMemberRead(_Base):
    user_id: int
    role: str
    joined_at: datetime


class InboxTeamRead(_Base):
    id: int
    name: str
    slug: str
    description: str | None = None
    owner_user_id: int
    created_at: datetime
    members: list[InboxTeamMemberRead] = Field(default_factory=list)


class InboxTeamCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    slug: str | None = None
    description: str | None = None

    @field_validator('slug', mode='before')
    @classmethod
    def _auto_slug(cls, v: str | None, info) -> str:
        if v:
            return v[:64]
        name = info.data.get('name', '')
        return _derive_slug(name) if name else ''


class InboxTeamMemberUpsert(BaseModel):
    user_id: int
    role: str = 'member'   # owner | admin | member


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


# ---------------------------------------------------------------------------
# Rules
# ---------------------------------------------------------------------------


class InboxRuleRead(_Base):
    id: int
    name: str
    enabled: bool
    priority: int
    trigger: str
    conditions: list[dict] | None = None
    actions: list[dict] | None = None
    created_at: datetime
    modified_at: datetime


class InboxRuleCreate(BaseModel):
    name: str = Field(min_length=1, max_length=128)
    enabled: bool = True
    priority: int = Field(default=100, ge=0, le=9999)
    trigger: str  # item_ingested | item_classified | star_synced
    conditions: list[dict] | None = None
    actions: list[dict] | None = None


class InboxRuleUpdate(BaseModel):
    name: str | None = Field(default=None, min_length=1, max_length=128)
    enabled: bool | None = None
    priority: int | None = Field(default=None, ge=0, le=9999)
    trigger: str | None = None
    conditions: list[dict] | None = None
    actions: list[dict] | None = None


class InboxRuleTestResult(BaseModel):
    rule_id: int
    matched: bool
    conditions_result: list[dict]   # [{field, op, value, passed}]
    actions_would_fire: list[dict]  # [{type, value}], only when matched=True
