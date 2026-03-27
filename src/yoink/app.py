"""Main orchestrator: loads plugins, builds PTB Application."""
from __future__ import annotations

import logging

from telegram.ext import Application, InlineQueryHandler

from yoink.core.bot.app import create_bot_app
from yoink.core.bot.admin import register as register_admin_commands
from yoink.core.bot.bot_commands import refresh_user_commands, set_default_commands, set_user_commands
from yoink.core.bot.commands import register as register_core_commands
from yoink.core.bot.forum import register as register_forum_handlers
from yoink.core.bot.group import register as register_group_commands
from yoink.core.bot.member import register as register_member_handlers
from yoink.core.config import CoreSettings
from yoink.core.db.engine import create_tables, get_session_factory, init_engine
from yoink.core.db.repos.bot_settings import BotSettingsRepo
from yoink.core.db.repos.groups import GroupRepo
from yoink.core.db.repos.users import UserRepo
from yoink.core.i18n.loader import register_locale_dir
from yoink.core.plugin import PluginContext, YoinkPlugin, load_plugins
from yoink.core.services.user_session import UserSessionService

logger = logging.getLogger(__name__)


async def _kick_proxy(bot: object) -> None:
    """Re-enable all configured proxies in the local Bot API server.

    tdlight-telegram-bot-api sometimes drops its MTProto connection after a
    restart and never reconnects even though the proxy is marked enabled.
    Cycling disable->enable forces it to re-establish the connection.
    Only runs when a custom base_url is configured (i.e. local bot API is used).
    """
    import httpx
    from telegram import Bot

    if not isinstance(bot, Bot):
        return

    base = bot.base_url  # e.g. "http://yoink-tg-bot-api:8082/bot<token>"
    # Only kick when using a non-official Bot API (local tdlight instance)
    if "api.telegram.org" in base:
        return

    async with httpx.AsyncClient(timeout=10) as client:
        try:
            r = await client.get(f"{base}/getProxies")
            data = r.json()
            proxies = data.get("result", []) if data.get("ok") else []
        except Exception as exc:
            logger.debug("_kick_proxy: getProxies failed: %s", exc)
            return

        for proxy in proxies:
            pid = proxy.get("id")
            if pid is None:
                continue
            try:
                await client.post(f"{base}/disableProxy", json={"proxy_id": pid})
                await client.post(f"{base}/enableProxy", json={"proxy_id": pid})
                logger.info("Proxy id=%d re-enabled (MTProto reconnect)", pid)
            except Exception as exc:
                logger.debug("_kick_proxy: enable failed for id=%d: %s", pid, exc)


def build_app(
    config: CoreSettings | None = None,
    plugins: list[YoinkPlugin] | None = None,
) -> Application:
    if config is None:
        config = CoreSettings()

    if plugins is None:
        plugins = load_plugins(config.yoink_plugins)

    app = create_bot_app(config)
    app.bot_data["config"] = config
    app.bot_data["plugins"] = plugins

    async def _post_init(application: Application) -> None:
        init_engine(config.database_url, echo=config.database_echo)

        # Import plugin models so they register in Base.metadata before create_all.
        # Base.metadata.create_all (called inside create_tables) covers everything.
        for plugin in plugins:
            plugin.get_models()
        await create_tables()

        session_factory = get_session_factory()

        # Core bot_data - owned by core, never overwritten by plugins.
        application.bot_data["session_factory"] = session_factory
        application.bot_data["user_repo"] = UserRepo(session_factory, owner_id=config.owner_id)
        application.bot_data["group_repo"] = GroupRepo(session_factory)
        application.bot_data["bot_settings_repo"] = BotSettingsRepo(session_factory)
        from yoink.core.db.repos.permissions import UserPermissionRepo
        application.bot_data["perm_repo"] = UserPermissionRepo(session_factory)

        # User-mode session - available if data/tg-bot-api/user.token exists.
        # Token is read fresh on every call; no startup failure if missing.
        user_session = UserSessionService(
            base_url=config.telegram_base_url.replace("/bot", ""),
            token_file=config.data_dir / "tg-bot-api" / "user.token",
        )
        application.bot_data["user_session"] = user_session
        if user_session.is_available():
            logger.info("User-mode session available")
        else:
            logger.info("User-mode session not configured (run: just tg-login)")

        for plugin in plugins:
            locale_dir = plugin.get_locale_dir()
            if locale_dir and locale_dir.exists():
                register_locale_dir(locale_dir)

        ctx = PluginContext(
            session_factory=session_factory,
            bot_data=application.bot_data,
            config=config,
            i18n=None,
        )

        # setup() lets each plugin populate its own namespaced bot_data keys.
        for plugin in plugins:
            await plugin.setup(ctx)

        for plugin in plugins:
            for spec in plugin.get_handlers():
                application.add_handler(spec.handler, group=spec.group)

        _register_inline_dispatcher(application, plugins)

        # Temporary: log all message updates to diagnose via_bot handler issue
        import telegram
        from telegram.ext import TypeHandler


        for plugin in plugins:
            for job in (plugin.get_jobs() or []):
                application.job_queue.run_repeating(
                    job.callback,
                    interval=job.interval,
                    first=job.first,
                    name=job.name or plugin.name,
                )

        plugin_commands = []
        for plugin in plugins:
            if hasattr(plugin, "get_commands"):
                plugin_commands.extend(plugin.get_commands())
        application.bot_data["plugin_commands"] = plugin_commands
        await set_default_commands(application.bot, plugin_commands=plugin_commands)

        class _State:
            bot = application.bot
            bot_data = application.bot_data

        # Refresh owner's per-chat commands on startup (owner has elevated role
        # and always needs a per-chat scope override).
        if config.owner_id:
            try:
                from sqlalchemy import select as sa_select
                from yoink.core.db.models import User
                async with session_factory() as _s:
                    owner = (await _s.execute(
                        sa_select(User).where(User.id == config.owner_id)
                    )).scalar_one_or_none()
                if owner:
                    await refresh_user_commands(
                        _State(), config.owner_id,
                        role=owner.role.value,
                        lang=owner.language,
                        session_factory=session_factory,
                    )
                    logger.info("Refreshed commands for owner %d (lang=%s)", config.owner_id, owner.language)
            except Exception as exc:
                logger.warning("Failed to refresh owner commands: %s", exc)

        await _kick_proxy(application.bot)

        me = await application.bot.get_me()
        logger.info("Bot started - @%s | plugins: %s", me.username, [p.name for p in plugins])

    app.post_init = _post_init
    register_core_commands(app)
    register_admin_commands(app)
    register_group_commands(app)
    register_forum_handlers(app)
    register_member_handlers(app)
    app.add_error_handler(_error_handler)
    return app


def _register_inline_dispatcher(
    application: Application,
    plugins: "list[YoinkPlugin]",
) -> None:
    """Build and register a single InlineQueryHandler that dispatches to plugins.

    Collects InlineHandlerSpecs from all plugins, sorts by descending priority,
    and routes each query to the first spec that matches.

    Matching order per spec:
      1. Explicit prefix: query starts with "<prefix> " or equals "<prefix>"
         - callback receives the text after the prefix (stripped)
      2. Pattern: regex matches the raw query
         - callback receives the full raw query
      3. Catch-all (no prefix, no pattern)
         - callback receives the full raw query

    The callback must return True if it answered the query, False to pass.
    If no spec handles the query, an empty answer is sent.
    """
    from yoink.core.plugin import InlineHandlerSpec

    specs: list[InlineHandlerSpec] = []
    for plugin in plugins:
        if hasattr(plugin, "get_inline_handlers"):
            specs.extend(plugin.get_inline_handlers())
    specs.sort(key=lambda s: -s.priority)

    if not specs:
        return

    async def _dispatch(update: object, context: object) -> None:
        import telegram
        if not isinstance(update, telegram.Update) or not update.inline_query:
            return
        q = update.inline_query
        raw = (q.query or "").strip()

        for spec in specs:
            query_text: str | None = None

            if spec.prefix is not None:
                if raw == spec.prefix:
                    query_text = ""
                elif raw.startswith(spec.prefix + " "):
                    query_text = raw[len(spec.prefix) + 1:].strip()

            if query_text is None and spec.pattern is not None:
                if spec.pattern.search(raw):
                    query_text = raw

            if query_text is None and spec.prefix is None and spec.pattern is None:
                query_text = raw

            if query_text is not None:
                if spec.access_policy is not None:
                    from yoink.core.bot.access import PermissionChecker
                    checker = PermissionChecker()
                    user = update.inline_query.from_user
                    result = await checker.check(
                        user_id=user.id,
                        chat=None,
                        thread_id=None,
                        policy=spec.access_policy,
                        context=context,
                        username=user.username,
                        first_name=user.first_name,
                    )
                    if not result.allowed:
                        logger.debug(
                            "Inline access denied: user=%d spec=%s reason=%s",
                            user.id, spec.callback.__name__, result.deny_reason,
                        )
                        continue
                try:
                    handled = await spec.callback(q, context, query_text)
                except Exception:
                    logger.exception("Inline handler %s raised", spec.callback)
                    handled = False
                if handled:
                    return

        await q.answer([], cache_time=0)

    application.add_handler(InlineQueryHandler(_dispatch))
    logger.debug("Inline dispatcher registered with %d spec(s)", len(specs))


async def _error_handler(update: object, context: object) -> None:
    import telegram.ext
    if isinstance(context, telegram.ext.CallbackContext):
        logger.warning("PTB error: %s", context.error, exc_info=context.error)
