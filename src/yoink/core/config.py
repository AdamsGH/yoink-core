"""Core settings. All env vars are read here."""
from __future__ import annotations

from pathlib import Path

from pydantic import field_validator, model_validator
from pydantic_settings import BaseSettings, SettingsConfigDict


class CoreSettings(BaseSettings):
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    # Telegram
    bot_token: str
    owner_id: int
    telegram_base_url: str = "https://api.telegram.org/bot"

    # Plugins (comma-separated entry-point names, e.g. "dl,stats")
    yoink_plugins: str = ""

    # Database
    database_url: str = "postgresql+asyncpg://yoink:yoink@localhost:5432/yoink"
    database_echo: bool = False

    # API
    api_port: int = 8000
    api_secret_key: str = "change-me-in-production"
    api_token_expire_minutes: int = 1440
    debug: bool = False
    # Enable /auth/dev endpoint only via this explicit flag - never tied to debug
    dev_auth_enabled: bool = False
    # JSON-formatted logs for production (one JSON object per line)
    json_logs: bool = False

    # Data directory (host-mounted volume, contains cookies, tg-bot-api session, etc.)
    data_dir: Path = Path("/app/data")

    # i18n
    default_language: str = "en"

    # Rate limiting (global defaults, plugins can override)
    rate_limit_per_minute: int = 5
    rate_limit_per_hour: int = 30
    rate_limit_per_day: int = 100

    # Logging
    log_channel: int | None = None
    log_exception_channel: int | None = None

    @field_validator("log_channel", "log_exception_channel", mode="before")
    @classmethod
    def _empty_str_to_none(cls, v: object) -> object:
        if isinstance(v, str) and v.strip() == "":
            return None
        return v

    @model_validator(mode="after")
    def _fill_log_channels(self) -> "CoreSettings":
        if self.log_exception_channel is None:
            self.log_exception_channel = self.log_channel
        return self
