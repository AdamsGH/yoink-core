"""InboxPlugin - implements YoinkPlugin protocol.

Phase 1+2 scope: ingestion via PTB / REST / web, LLM categorisation through
yoink-insight services, GitHub stars viewer with folder organisation, rules
engine. See TODO-c66b1c02 for the full plan.

This module is the protocol entry point. Real work lives under:
- services/         business logic (ingest, enrich, classify, gh_*, rules)
- storage/          SQLAlchemy models + repos
- api/router.py     FastAPI router mounted at /api/v1/inbox
- commands/         PTB handlers
- worker.py         ARQ WorkerSettings
"""
from __future__ import annotations

from pathlib import Path

from fastapi import APIRouter

from yoink.core.plugin import (
    FeatureSpec,
    InlineHandlerSpec,
    JobSpec,
    PluginContext,
    SidebarEntry,
    WebManifest,
    WebPage,
)
from yoink_inbox.config import InboxConfig


class InboxPlugin:
    name = "inbox"
    version = "0.1.0"

    def __init__(self) -> None:
        self._config = InboxConfig()

    def get_config_class(self) -> type[InboxConfig]:
        return InboxConfig

    def get_models(self) -> list:
        # Imported lazily so Alembic env doesn't have to load services.
        from yoink_inbox.storage.models import (
            InboxCategory,
            InboxGhFolder,
            InboxGhFolderMember,
            InboxGhStar,
            InboxItem,
            InboxItemCategory,
            InboxItemToStar,
            InboxRule,
            InboxTeam,
            InboxTeamMember,
        )

        return [
            InboxTeam,
            InboxTeamMember,
            InboxItem,
            InboxCategory,
            InboxItemCategory,
            InboxGhStar,
            InboxGhFolder,
            InboxGhFolderMember,
            InboxItemToStar,
            InboxRule,
        ]

    def get_handlers(self) -> list:
        from yoink_inbox.commands import get_handler_specs

        return get_handler_specs()

    def get_inline_handlers(self) -> list[InlineHandlerSpec]:
        return []

    def get_routes(self) -> APIRouter | None:
        from yoink_inbox.api.router import router

        return router

    def get_locale_dir(self) -> Path | None:
        return Path(__file__).parent / "i18n" / "locales"

    def get_web_manifest(self) -> WebManifest:
        return WebManifest(
            pages=[
                WebPage(
                    path="/inbox",
                    sidebar=SidebarEntry(
                        label="Inbox",
                        icon="Inbox",
                        path="/inbox",
                        section="main",
                    ),
                ),
                WebPage(
                    path="/inbox/stars",
                    sidebar=SidebarEntry(
                        label="GitHub Stars",
                        icon="Star",
                        path="/inbox/stars",
                        section="main",
                    ),
                ),
                WebPage(
                    path="/inbox/rules",
                    sidebar=SidebarEntry(
                        label="Inbox Rules",
                        icon="Filter",
                        path="/inbox/rules",
                        section="main",
                    ),
                ),
                WebPage(
                    path="/inbox/teams",
                    sidebar=SidebarEntry(
                        label="Inbox Teams",
                        icon="Users",
                        path="/inbox/teams",
                        section="main",
                    ),
                ),
            ]
        )

    def get_commands(self) -> list:
        import yaml

        from yoink.core.plugin import CommandSpec

        locales_dir = Path(__file__).parent / "i18n" / "locales"
        en_path = locales_dir / "en.yml"
        if not en_path.exists():
            return []
        en_data = yaml.safe_load(en_path.read_text()) or {}

        lang_descriptions: dict[str, dict[str, str]] = {}
        for locale_file in locales_dir.glob("*.yml"):
            lang = locale_file.stem
            if lang == "en":
                continue
            try:
                loc_data = yaml.safe_load(locale_file.read_text()) or {}
                for entry in loc_data.get("commands") or []:
                    cmd = entry.get("command")
                    desc = entry.get("description")
                    if cmd and desc:
                        lang_descriptions.setdefault(cmd, {})[lang] = desc
            except Exception:
                pass

        return [
            CommandSpec(
                command=entry["command"],
                description=entry["description"],
                min_role=entry.get("min_role", "user"),
                scope=entry.get("scope", "default"),
                descriptions=lang_descriptions.get(entry["command"], {}),
                required_feature=entry.get("required_feature"),
            )
            for entry in (en_data.get("commands") or [])
        ]

    def get_features(self) -> list[FeatureSpec]:
        return [
            FeatureSpec(
                plugin="inbox",
                feature="ingest",
                label="Inbox Ingest",
                description="Save links into the inbox (via bot, web, or REST).",
                default_min_role="user",
            ),
            FeatureSpec(
                plugin="inbox",
                feature="classify",
                label="AI Categorisation",
                description="Automatically categorise saved links with an LLM.",
                default_min_role=None,
            ),
            FeatureSpec(
                plugin="inbox",
                feature="share",
                label="Share Categories",
                description="Share categories with an inbox team.",
                default_min_role="user",
            ),
            FeatureSpec(
                plugin="inbox",
                feature="gh_sync",
                label="GitHub Stars Sync",
                description="Sync the user's starred GitHub repositories into the inbox viewer.",
                default_min_role=None,
            ),
            FeatureSpec(
                plugin="inbox",
                feature="gh_organise",
                label="AI-Organise Stars",
                description="Batch-organise starred repositories into folders with an LLM.",
                default_min_role=None,
            ),
            FeatureSpec(
                plugin="inbox",
                feature="gh_write",
                label="Star/Unstar From Yoink",
                description="Star or unstar repositories from the yoink UI. Requires opt-in "
                "public_repo OAuth upgrade.",
                default_min_role=None,
            ),
            FeatureSpec(
                plugin="inbox",
                feature="admin",
                label="Inbox Admin",
                description="Manage the inbox of any user.",
                default_min_role="admin",
            ),
        ]

    def get_jobs(self) -> list[JobSpec] | None:
        # Periodic GH stars sync wakes up and enqueues per-user ARQ jobs.
        # Real callback wired in once services/gh_stars.py lands; until then
        # JobSpec list is empty so the bot starts cleanly.
        return []

    async def setup(self, ctx: PluginContext) -> None:
        """Populate bot_data with inbox-specific services."""
        # Repos and services are wired here so commands / routes / worker
        # functions share a single instance per process. Imports are local to
        # keep `import yoink_inbox` cheap for Alembic env discovery.
        ctx.bot_data["inbox_config"] = self._config

        # TODO: wire the following once their modules land:
        # - InboxItemRepo, InboxCategoryRepo, InboxGhStarRepo, InboxRuleRepo,
        #   InboxTeamRepo from yoink_inbox.storage.repos
        # - IngestService, EnrichService, ClassifyService, GhStarsService,
        #   RulesEngine from yoink_inbox.services.*
        # - ARQ Redis pool from yoink_inbox.worker (so REST handlers can
        #   enqueue jobs without owning the connection).
