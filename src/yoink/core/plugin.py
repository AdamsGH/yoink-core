"""Plugin protocol and registry.

Plugins are discovered via Python entry_points (group "yoink.plugins").
Only plugins listed in YOINK_PLUGINS env var are activated.

Architecture notes:
- All plugin models inherit from yoink.core.db.base.Base (single DeclarativeBase).
  Importing a model module is sufficient to register its tables in Base.metadata.
  create_tables() calls Base.metadata.create_all - no per-plugin metadata needed.
- get_models() exists solely so Alembic env.py can import plugin models before
  autogenerate. It is NOT used at runtime for table creation.
- Each plugin has a namespaced bot_data key for its own services, e.g.
  "dl_user_repo", "stats_message_repo". The core keys "user_repo",
  "group_repo", "bot_settings_repo", "session_factory", "config" are
  owned by core and must never be overwritten by plugins.
"""
from __future__ import annotations

import importlib.metadata
import logging
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol, runtime_checkable

from fastapi import APIRouter
from pydantic_settings import BaseSettings
from telegram.ext import BaseHandler

logger = logging.getLogger(__name__)


@dataclass
class FeatureSpec:
    """A gated feature declared by a plugin.

    Plugins return a list of FeatureSpec from get_features().
    Core uses this list to:
    - Show available features in GET /features (admin UI).
    - Enforce access checks when AccessPolicy.feature is set.

    default_min_role: if set, users with this role or higher get access
    without an explicit user_permissions row. None means the feature is
    always gated by an explicit grant (owner always passes regardless).
    """
    plugin: str
    feature: str
    label: str
    description: str = ""
    default_min_role: str | None = None  # "user" | "moderator" | "admin" | "owner" | None


# Global registry populated by load_plugins()
_feature_registry: list[FeatureSpec] = []


def register_features(features: list[FeatureSpec]) -> None:
    _feature_registry.extend(features)


def get_all_features() -> list[FeatureSpec]:
    return list(_feature_registry)


@dataclass
class HandlerSpec:
    """Wraps a PTB handler with metadata for registration."""
    handler: BaseHandler
    group: int = 0
    scope: str = "all"  # "private" | "group" | "all"


@dataclass
class JobSpec:
    """Recurring job specification."""
    callback: Any
    interval: float
    first: float = 0.0
    name: str = ""


@dataclass
class CommandSpec:
    """A bot command with its visibility rules.

    Roles follow the same hierarchy as UserRole in core:
      user < moderator < admin < owner

    Scopes:
      default     - private chats, shown in the autocomplete menu
      groups      - shown in group chats
      private     - private only, not shown in groups

    descriptions maps language codes to localized command descriptions.
    The value of `description` is used as the fallback (default locale).
    Pass additional translations via descriptions={'ru': '...', 'de': '...'}.

    required_feature: if set as "plugin:feature", the command is only shown
    to users who have effective access to that feature (role or explicit grant).
    """
    command: str
    description: str
    min_role: str = "user"   # "user" | "moderator" | "admin" | "owner"
    scope: str = "default"   # "default" | "groups" | "private"
    descriptions: dict[str, str] = field(default_factory=dict)
    required_feature: str | None = None  # "plugin:feature" e.g. "insight:summary"


@dataclass
class InlineHandlerSpec:
    """An inline query handler registered by a plugin.

    The dispatcher in app.py collects all InlineHandlerSpecs from all plugins,
    sorts them by descending priority, and routes each incoming inline query to
    the first spec whose prefix or pattern matches.

    Matching rules (checked in order):
      1. If ``prefix`` is set and the query starts with "<prefix> " (or equals
         the prefix exactly), the handler is called with the query text after
         the prefix stripped.
      2. If ``pattern`` is set and it matches the full query, the handler is
         called with the full query.
      3. If neither ``prefix`` nor ``pattern`` is set, the spec acts as a
         catch-all and is always offered (good for a default text-search handler).

    The callback signature is:
        async def handler(query: InlineQuery, context, query_text: str) -> bool

    It must return True if it produced a response (answer() was called), or
    False to let the next matching spec try.
    """
    callback: Any
    priority: int = 0
    prefix: str | None = None
    pattern: "re.Pattern[str] | None" = None
    access_policy: "Any | None" = None  # AccessPolicy | None


@dataclass
class SidebarEntry:
    label: str
    icon: str
    path: str
    section: str = "main"  # "main" | "admin"
    min_role: str = "user"


@dataclass
class WebPage:
    path: str
    sidebar: SidebarEntry | None = None


@dataclass
class WebManifest:
    pages: list[WebPage] = field(default_factory=list)


@dataclass
class PluginContext:
    """Core services injected into every plugin at startup.

    bot_data keys guaranteed by core before setup() is called:
      "session_factory", "config", "user_repo", "group_repo", "bot_settings_repo"
    Plugins must use namespaced keys for their own services.
    """
    session_factory: Any   # async_sessionmaker
    bot_data: dict[str, Any]
    config: Any            # CoreSettings
    i18n: Any              # reserved for future use


@runtime_checkable
class YoinkPlugin(Protocol):
    """Contract every plugin must satisfy.

    Minimal implementation:
        name = "myplugin"
        version = "0.1.0"
        def get_config_class(self): return None
        def get_models(self): return []
        def get_handlers(self): return []
        def get_inline_handlers(self): return []
        def get_routes(self): return None
        def get_locale_dir(self): return None
        def get_web_manifest(self): return None
        def get_jobs(self): return None
        async def setup(self, ctx): pass
    """

    name: str
    version: str

    def get_config_class(self) -> type[BaseSettings] | None:
        """Plugin-specific Pydantic settings class, or None."""
        ...

    def get_models(self) -> list[type]:
        """ORM model classes - imported so Alembic can discover their tables.
        Not used for runtime table creation (Base.metadata handles that)."""
        ...

    def get_handlers(self) -> list[HandlerSpec]:
        """PTB handlers to register in the Application."""
        ...

    def get_inline_handlers(self) -> list[InlineHandlerSpec]:
        """Inline query handlers to register in the central dispatcher."""
        ...

    def get_routes(self) -> APIRouter | None:
        """FastAPI router, mounted at /api/v1/{plugin.name}/."""
        ...

    def get_locale_dir(self) -> Path | None:
        """Directory with en.yml, ru.yml etc., merged into global i18n."""
        ...

    def get_web_manifest(self) -> WebManifest | None:
        """Frontend pages and sidebar entries for the mini app."""
        ...

    def get_jobs(self) -> list[JobSpec] | None:
        """Recurring PTB JobQueue jobs."""
        ...

    def get_features(self) -> list[FeatureSpec]:
        """Gated features this plugin exposes. Core registers them globally."""
        ...

    def get_commands(self) -> list[CommandSpec]:
        """Commands to register with Telegram, with role and scope metadata."""
        ...

    def get_help_section(self, role: str, lang: str, granted_features: set[str] | None = None) -> str:
        """Return HTML help text for the given role, or empty string if none."""
        ...

    async def setup(self, ctx: PluginContext) -> None:
        """Called once at startup after core bot_data is ready.

        Use this to populate ctx.bot_data with plugin-specific services.
        Must be idempotent. Must not overwrite core bot_data keys.
        """
        ...


def _discover_plugins(enabled: list[str]) -> list[YoinkPlugin]:
    eps = importlib.metadata.entry_points(group="yoink.plugins")
    plugins: list[YoinkPlugin] = []
    for ep in eps:
        if ep.name not in enabled:
            continue
        try:
            cls = ep.load()
            instance = cls()
            plugins.append(instance)
            logger.info("Loaded plugin: %s v%s", instance.name, instance.version)
            if hasattr(instance, "get_features"):
                try:
                    register_features(instance.get_features())
                except Exception:
                    logger.exception("Failed to register features for plugin %s", ep.name)
        except Exception:
            logger.exception("Failed to load plugin %s", ep.name)
    return plugins


def load_plugins(names: str) -> list[YoinkPlugin]:
    """Parse YOINK_PLUGINS comma-separated string and discover matching plugins."""
    enabled = [n.strip() for n in names.split(",") if n.strip()]
    if not enabled:
        return []
    return _discover_plugins(enabled)
