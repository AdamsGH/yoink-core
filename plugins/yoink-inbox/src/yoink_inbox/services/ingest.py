"""Inbox ingest service.

Single entry point: `ingest_url`. Responsible for URL normalisation, dedup
against the user's existing items, inserting the `pending` row, and
enqueueing the enrich job on ARQ. Subsequent pipeline stages (enrich,
classify, gh_detect) live in their own service modules and run on the
worker, not in the caller's request loop.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import TYPE_CHECKING
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from yoink_inbox.storage.models import InboxItem

if TYPE_CHECKING:
    from arq.connections import ArqRedis

logger = logging.getLogger(__name__)


# Query-parameter prefixes / exact keys we strip during normalisation. Order
# is not important (set membership). Same list karakeep uses, plus the
# `mc_*` mailchimp tags and `_hsenc` HubSpot tracking.
_TRACKING_PREFIXES: tuple[str, ...] = (
    "utm_",
    "mc_",
    "_hsenc",
    "_hsmi",
    "hsCtaTracking",
)
_TRACKING_EXACT: frozenset[str] = frozenset(
    {
        "gclid",
        "fbclid",
        "yclid",
        "msclkid",
        "dclid",
        "igshid",
        "ref",
        "ref_src",
        "ref_url",
        "feature",
        "si",
        "spm",
    }
)


@dataclass(slots=True)
class IngestResult:
    """Outcome of an `ingest_url` call.

    `created` is True only when a new row was inserted; False means the URL
    was deduped against an existing row (still a success from the caller's
    perspective). `enrich_job_id` is set when an enrich job was enqueued.
    """

    item_id: int
    normalized_url: str
    created: bool
    enrich_job_id: str | None = None


def normalize_url(url: str) -> str:
    """Canonicalise a URL for dedup.

    Rules:
    - lowercase scheme + host
    - strip default ports (:80 / :443)
    - strip fragment
    - drop tracking query params (utm_*, gclid, fbclid, etc.)
    - sort remaining query keys
    - strip trailing slash on path unless path is empty / root
    """
    s = urlsplit(url.strip())
    if not s.scheme or not s.netloc:
        # Bare host? Fall back to https:// + retry one level.
        if "://" not in url:
            return normalize_url(f"https://{url.strip()}")
        # Cannot normalise; return raw lowercased.
        return url.strip().lower()

    scheme = s.scheme.lower()
    netloc = s.hostname or ""
    netloc = netloc.lower()
    if s.port and not (
        (scheme == "http" and s.port == 80) or (scheme == "https" and s.port == 443)
    ):
        netloc = f"{netloc}:{s.port}"

    path = s.path or ""
    if len(path) > 1 and path.endswith("/"):
        path = path.rstrip("/")

    kept = [
        (k, v)
        for k, v in parse_qsl(s.query, keep_blank_values=False)
        if not _is_tracking_key(k)
    ]
    kept.sort()
    query = urlencode(kept, doseq=True)

    return urlunsplit((scheme, netloc, path, query, ""))


def _is_tracking_key(key: str) -> bool:
    k = key.lower()
    if k in _TRACKING_EXACT:
        return True
    return any(k.startswith(p) for p in _TRACKING_PREFIXES)


def detect_kind(normalized: str) -> str:
    """Classify a URL into one of the InboxItem.kind values from its shape.

    Cheap heuristic only. LLM classification refines this later.
    """
    s = urlsplit(normalized)
    host = (s.hostname or "").lower()
    path_parts = [p for p in s.path.split("/") if p]

    if host in {"github.com", "www.github.com"} and len(path_parts) >= 2:
        # /owner/repo or /owner/repo/anything
        return "github_repo"
    if host in {"youtube.com", "www.youtube.com", "youtu.be", "m.youtube.com"}:
        return "video"
    if host in {"vimeo.com", "www.vimeo.com"}:
        return "video"
    return "link"


async def ingest_url(
    session: AsyncSession,
    *,
    user_id: int,
    url: str,
    source: str = "bot",
    arq: "ArqRedis | None" = None,
) -> IngestResult:
    """Insert an inbox item for `url` if not already present.

    Caller controls the session lifecycle (commit / rollback) so this fits
    cleanly inside larger transactions. Enrich enqueue happens after the
    session commits in the caller; we only return the prepared job-id so the
    caller can decide when to fire it.
    """
    normalized = normalize_url(url)
    kind = detect_kind(normalized)

    existing = await session.scalar(
        select(InboxItem).where(
            InboxItem.user_id == user_id,
            InboxItem.normalized_url == normalized,
        )
    )
    if existing is not None:
        logger.info(
            "inbox.ingest dedup user_id=%s item_id=%s url=%s",
            user_id, existing.id, normalized,
        )
        return IngestResult(
            item_id=existing.id,
            normalized_url=normalized,
            created=False,
        )

    item = InboxItem(
        user_id=user_id,
        url=url,
        normalized_url=normalized,
        kind=kind,
        source=source,
        status="pending",
    )
    session.add(item)
    await session.flush()  # populate item.id without committing

    enrich_job_id: str | None = None
    if arq is not None:
        # _job_id keys are idempotent: a duplicate enqueue collapses into the
        # first one until it finishes. Stage tagged so a future re-classify
        # does not collide with the initial enrich.
        job = await arq.enqueue_job(
            "enrich_item",
            item.id,
            _job_id=f"inbox:enrich:{item.id}",
            _queue_name="inbox:default",
        )
        if job is not None:
            enrich_job_id = job.job_id

    logger.info(
        "inbox.ingest created user_id=%s item_id=%s kind=%s url=%s enrich_job=%s",
        user_id, item.id, kind, normalized, enrich_job_id,
    )
    return IngestResult(
        item_id=item.id,
        normalized_url=normalized,
        created=True,
        enrich_job_id=enrich_job_id,
    )
