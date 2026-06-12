"""Inbox plugin configuration."""
from __future__ import annotations

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class InboxConfig(BaseSettings):
    """Inbox plugin settings.

    All values are read from the project-wide `.env` (single source of truth).
    Keep field names lowercase, matching the convention in `CoreSettings` /
    `InsightConfig`.
    """

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        case_sensitive=False,
        extra="ignore",
    )

    # ---- Ingest / enrich -------------------------------------------------
    inbox_max_content_chars: int = Field(
        default=12000,
        description="Hard cap on extracted body length stored in inbox_items.content_text.",
    )
    inbox_url_dedup_window_days: int = Field(
        default=365,
        description="Re-ingesting the same normalized_url within this window dedups silently. "
        "Older identical URLs become a new item.",
    )

    # ---- LLM classification ----------------------------------------------
    inbox_classify_max_categories: int = Field(default=3)
    inbox_classify_allow_new_category: bool = Field(default=True)
    inbox_classify_token_budget: int = Field(
        default=8000,
        description="Max tokens of EXTRACTED_CONTENT included in the classify prompt. "
        "Truncation is character-based with a tiktoken-driven safety margin.",
    )

    # ---- ARQ worker ------------------------------------------------------
    inbox_redis_url: str = Field(default="redis://redis:6379/3")
    inbox_arq_enrich_max_jobs: int = Field(default=3)
    inbox_arq_classify_max_jobs: int = Field(default=5)
    inbox_arq_gh_sync_max_jobs: int = Field(default=1)
    inbox_arq_gh_organise_max_jobs: int = Field(default=2)
    inbox_arq_max_tries: int = Field(default=3)

    # ---- GitHub stars sync ----------------------------------------------
    inbox_gh_sync_interval_hours: int = Field(default=24)
    inbox_gh_sync_page_size: int = Field(default=100)
    inbox_gh_organise_batch_size: int = Field(default=10)
    inbox_gh_organise_concurrency: int = Field(default=5)

    # ---- Telegram surface -----------------------------------------------
    inbox_recent_items_limit: int = Field(
        default=20,
        description="How many items /inbox shows by default.",
    )
    inbox_recent_stars_limit: int = Field(default=20)
