"""Periodic PTB job callbacks for the inbox plugin.

Registered via InboxPlugin.get_jobs(); executed by the PTB job queue
(not ARQ) so they run in the bot process, not the worker. Their job is
lightweight: query the DB for users who need a sync and enqueue ARQ
tasks. Heavy lifting stays in the ARQ worker.
"""
from __future__ import annotations

import logging
from datetime import UTC, datetime, timedelta

from sqlalchemy import select

logger = logging.getLogger(__name__)


async def scheduled_gh_sync(context) -> None:  # context: telegram.ext.CallbackContext
    """Enqueue a sync_user_stars ARQ job for every user who qualifies.

    Qualification: has a non-null github_token in insight_user_settings
    AND (has never synced OR last sync is older than inbox_gh_sync_interval_hours).

    Skips gracefully when the ARQ pool is unavailable (Redis down) or when
    insight models are not installed (yoink-insight not in the plugin set).
    """
    bot_data = context.application.bot_data
    arq = bot_data.get("inbox_arq_pool")
    if arq is None:
        logger.warning("inbox.scheduled_gh_sync: ARQ pool unavailable, skipping")
        return

    session_factory = bot_data.get("session_factory")
    if session_factory is None:
        logger.warning("inbox.scheduled_gh_sync: session_factory not in bot_data, skipping")
        return

    config = bot_data.get("inbox_config")
    interval_hours: int = getattr(config, "inbox_gh_sync_interval_hours", 24)
    cutoff = datetime.now(UTC) - timedelta(hours=interval_hours)

    try:
        from yoink_insight.storage.models import InsightUserSettings
    except ImportError:
        logger.debug("inbox.scheduled_gh_sync: yoink-insight not installed, skipping")
        return

    from yoink_inbox.storage.models import InboxGhSyncState

    async with session_factory() as session:
        # All users with a GitHub token.
        token_rows = await session.scalars(
            select(InsightUserSettings.user_id).where(
                InsightUserSettings.github_token.is_not(None),
                InsightUserSettings.github_token != "",
            )
        )
        all_user_ids: list[int] = list(token_rows.all())

        if not all_user_ids:
            logger.debug("inbox.scheduled_gh_sync: no users with github_token")
            return

        # Sync states for those users (may be absent if they never synced).
        state_rows = await session.scalars(
            select(InboxGhSyncState).where(
                InboxGhSyncState.user_id.in_(all_user_ids)
            )
        )
        states: dict[int, InboxGhSyncState] = {s.user_id: s for s in state_rows.all()}

    enqueued = 0
    for user_id in all_user_ids:
        state = states.get(user_id)
        if state is not None and state.last_synced_at is not None:
            if state.last_synced_at >= cutoff:
                logger.debug(
                    "inbox.scheduled_gh_sync: user_id=%s synced recently (%s), skip",
                    user_id, state.last_synced_at,
                )
                continue
        try:
            await arq.enqueue_job("sync_user_stars", user_id, _queue_name="inbox:default")
            enqueued += 1
            logger.info("inbox.scheduled_gh_sync: enqueued sync for user_id=%s", user_id)
        except Exception:
            logger.exception("inbox.scheduled_gh_sync: enqueue failed for user_id=%s", user_id)

    logger.info("inbox.scheduled_gh_sync: enqueued %d / %d users", enqueued, len(all_user_ids))
