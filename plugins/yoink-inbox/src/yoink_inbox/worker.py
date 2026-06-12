"""ARQ worker entry point.

`docker compose run yoink-arq-worker` boots `arq yoink_inbox.worker.WorkerSettings`.
Each pipeline stage (enrich, classify, gh_sync, gh_organise) is a function
listed in `functions`. Stubs raise NotImplementedError until their service
modules land; the worker still starts so the deployment plumbing can be
verified end-to-end before the logic is written.
"""
from __future__ import annotations

import logging

from arq.connections import RedisSettings
from arq.worker import func

from yoink_inbox.config import InboxConfig

logger = logging.getLogger(__name__)
_config = InboxConfig()


async def enrich_item(ctx: dict, item_id: int) -> None:
    """Fetch metadata + readable content for an inbox item.

    Wraps yoink_insight.services.fetch.fetch_web_content. Service module
    lands in the next commit; until then this is a noop stub that logs and
    returns so ARQ wiring is observable.
    """
    logger.info("inbox.enrich_item stub item_id=%s", item_id)
    # TODO: from yoink_inbox.services.enrich import run_enrich
    # await run_enrich(ctx, item_id)


async def classify_item(ctx: dict, item_id: int) -> None:
    """Run hybrid LLM categorisation on an enriched item."""
    logger.info("inbox.classify_item stub item_id=%s", item_id)
    # TODO: from yoink_inbox.services.classify import run_classify


async def sync_user_stars(ctx: dict, user_id: int) -> None:
    """Snapshot the user's full GitHub starred list."""
    logger.info("inbox.sync_user_stars stub user_id=%s", user_id)
    # TODO: from yoink_inbox.services.gh_stars import run_sync


async def organise_stars_batch(ctx: dict, user_id: int) -> None:
    """Batch AI-organise the user's synced stars into folders."""
    logger.info("inbox.organise_stars_batch stub user_id=%s", user_id)
    # TODO: from yoink_inbox.services.gh_stars import run_organise


async def startup(ctx: dict) -> None:
    """ARQ startup hook. Place to instantiate shared HTTP clients later."""
    logger.info("inbox.worker startup")
    ctx["config"] = _config


async def shutdown(ctx: dict) -> None:
    logger.info("inbox.worker shutdown")


class WorkerSettings:
    """ARQ settings discovered by `arq yoink_inbox.worker.WorkerSettings`.

    Single queue `inbox:default`, single worker process. Per-stage rate
    limiting is enforced inside each function via asyncio.Semaphore once
    the real implementations land. If a heavy stage (gh_organise, LLM
    batches) needs its own pool later, add a second compose service with
    a different `queue_name` and route those enqueues with `_queue_name=`.
    """

    redis_settings = RedisSettings.from_dsn(_config.inbox_redis_url)
    queue_name = "inbox:default"
    functions = [
        func(enrich_item, name="enrich_item", max_tries=_config.inbox_arq_max_tries),
        func(classify_item, name="classify_item", max_tries=_config.inbox_arq_max_tries),
        func(sync_user_stars, name="sync_user_stars", max_tries=_config.inbox_arq_max_tries),
        func(
            organise_stars_batch,
            name="organise_stars_batch",
            max_tries=_config.inbox_arq_max_tries,
        ),
    ]
    on_startup = startup
    on_shutdown = shutdown
    max_jobs = max(
        _config.inbox_arq_enrich_max_jobs,
        _config.inbox_arq_classify_max_jobs,
        _config.inbox_arq_gh_sync_max_jobs,
        _config.inbox_arq_gh_organise_max_jobs,
    )
    job_timeout = 300
    keep_result = 3600
