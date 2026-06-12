"""Enrich an inbox item with title, summary metadata, content text, and assets.

Single entry: `run_enrich(session_factory, item_id)`. Builds its own session
because it runs from the ARQ worker, not a request handler. Strategy:

1. Load the inbox item; bail if it is not in a state that wants enrichment
   (already enriched / archived / failed).
2. Call `yoink_insight.services.fetch.fetch_web_content` for body text. The
   user's stored GitHub token is passed when available so private repos and
   high rate limit windows work transparently.
3. For non-GitHub URLs do a cheap parallel OpenGraph scrape so we capture
   og:image / favicon / og:site_name (publisher) / og:description for the
   inbox card preview. Failure here is non-fatal.
4. For GitHub repo URLs call /repos/{owner}/{repo} directly for description,
   owner avatar, and homepage. Again non-fatal.
5. Write title/summary-stub/content_text/author/publisher/og_image_url/
   favicon_url back to the row, set status=enriched, then enqueue classify.

Classify enqueue is fire-and-forget here; we get the ARQ pool out of `ctx`.
"""
from __future__ import annotations

import asyncio
import logging
import re
from dataclasses import dataclass
from typing import TYPE_CHECKING
from urllib.parse import urlsplit

import httpx
from sqlalchemy import select

if TYPE_CHECKING:
    from arq.connections import ArqRedis
    from sqlalchemy.ext.asyncio import async_sessionmaker

logger = logging.getLogger(__name__)

_OG_TIMEOUT = httpx.Timeout(10.0, connect=5.0)
_OG_USER_AGENT = (
    "Mozilla/5.0 (compatible; yoink-inbox/0.1; +https://github.com/)"
)


@dataclass(slots=True)
class OpenGraph:
    """Minimal OG tag bag we care about for inbox previews."""

    title: str | None = None
    description: str | None = None
    image: str | None = None
    site_name: str | None = None
    author: str | None = None
    favicon: str | None = None


_OG_TAG_RE = re.compile(
    r"""<meta\s+[^>]*?
        (?:property|name)\s*=\s*["'](?P<key>og:[^"']+|twitter:[^"']+|author|description)["']
        [^>]*?
        content\s*=\s*["'](?P<val>[^"']*)["']
        [^>]*?>""",
    re.IGNORECASE | re.VERBOSE | re.DOTALL,
)
_TITLE_RE = re.compile(r"<title[^>]*>(.*?)</title>", re.IGNORECASE | re.DOTALL)
_FAVICON_RE = re.compile(
    r'<link\s+[^>]*?rel=["\'](?:shortcut\s+icon|icon)["\'][^>]*?href=["\']([^"\']+)["\']',
    re.IGNORECASE,
)


async def _scrape_opengraph(client: httpx.AsyncClient, url: str) -> OpenGraph:
    """Cheap OG scrape. Returns empty OpenGraph on any failure."""
    og = OpenGraph()
    try:
        r = await client.get(url, headers={"User-Agent": _OG_USER_AGENT})
        if r.status_code >= 400:
            return og
        html = r.text[:262144]  # cap to 256 KB; anything past that is overkill
    except (httpx.HTTPError, httpx.InvalidURL) as exc:
        logger.debug("og scrape failed url=%s: %r", url, exc)
        return og

    for m in _OG_TAG_RE.finditer(html):
        key = m.group("key").lower()
        val = m.group("val").strip()
        if not val:
            continue
        if key in {"og:title", "twitter:title"} and not og.title:
            og.title = val
        elif key in {"og:description", "twitter:description", "description"} and not og.description:
            og.description = val
        elif key in {"og:image", "twitter:image", "twitter:image:src"} and not og.image:
            og.image = val
        elif key == "og:site_name" and not og.site_name:
            og.site_name = val
        elif key == "author" and not og.author:
            og.author = val

    if not og.title:
        if (tm := _TITLE_RE.search(html)) is not None:
            og.title = re.sub(r"\s+", " ", tm.group(1)).strip() or None

    if (fm := _FAVICON_RE.search(html)) is not None:
        og.favicon = _absolutise(url, fm.group(1))
    else:
        og.favicon = _default_favicon(url)

    return og


def _absolutise(base_url: str, href: str) -> str | None:
    if not href:
        return None
    if href.startswith(("http://", "https://", "//")):
        return f"https:{href}" if href.startswith("//") else href
    s = urlsplit(base_url)
    if href.startswith("/"):
        return f"{s.scheme}://{s.netloc}{href}"
    return f"{s.scheme}://{s.netloc}/{href}"


def _default_favicon(url: str) -> str | None:
    s = urlsplit(url)
    if not s.netloc:
        return None
    return f"{s.scheme}://{s.netloc}/favicon.ico"


@dataclass(slots=True)
class GhRepoMeta:
    description: str | None = None
    homepage: str | None = None
    owner_avatar: str | None = None
    stargazers: int | None = None
    language: str | None = None


async def _fetch_github_repo(
    client: httpx.AsyncClient, owner: str, repo: str, token: str | None
) -> GhRepoMeta:
    """Hit /repos/{owner}/{repo}. Returns empty meta on failure."""
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": _OG_USER_AGENT,
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    try:
        r = await client.get(
            f"https://api.github.com/repos/{owner}/{repo}", headers=headers,
        )
        if r.status_code >= 400:
            return GhRepoMeta()
        data = r.json()
    except (httpx.HTTPError, ValueError) as exc:
        logger.debug("gh repo meta failed %s/%s: %r", owner, repo, exc)
        return GhRepoMeta()
    return GhRepoMeta(
        description=data.get("description"),
        homepage=data.get("homepage") or None,
        owner_avatar=(data.get("owner") or {}).get("avatar_url"),
        stargazers=data.get("stargazers_count"),
        language=data.get("language"),
    )


def _parse_github_repo(normalized_url: str) -> tuple[str, str] | None:
    s = urlsplit(normalized_url)
    if (s.hostname or "").lower() not in {"github.com", "www.github.com"}:
        return None
    parts = [p for p in s.path.split("/") if p]
    if len(parts) < 2:
        return None
    # Strip common suffix paths so /foo/bar/tree/main still resolves to foo/bar
    return parts[0], parts[1]


async def _load_github_token(
    session_factory: "async_sessionmaker", user_id: int
) -> str | None:
    """Pull the user's stored GitHub token from yoink-insight settings.

    Returns None if insight is not enabled or the user never linked GitHub.
    """
    try:
        from yoink_insight.storage.models import InsightUserSettings
    except ImportError:
        return None
    async with session_factory() as s:
        settings = await s.scalar(
            select(InsightUserSettings).where(
                InsightUserSettings.user_id == user_id
            )
        )
        return settings.github_token if settings else None


async def run_enrich(
    session_factory: "async_sessionmaker",
    item_id: int,
    *,
    arq: "ArqRedis | None" = None,
    max_chars: int = 12000,
) -> None:
    """Enrich one inbox item and (optionally) enqueue the classify follow-up.

    Idempotent in the sense that re-running on an already-enriched item just
    rewrites the same fields. Status transitions: pending -> enriched, with
    crawl_status='success' or 'failed' on the body.
    """
    # Local import keeps Alembic env free from service deps.
    from yoink_insight.services import fetch as insight_fetch

    from yoink_inbox.storage.models import InboxItem

    async with session_factory() as session:
        item = await session.get(InboxItem, item_id)
        if item is None:
            logger.warning("inbox.enrich missing item_id=%s", item_id)
            return
        if item.status in {"archived", "failed"}:
            logger.info("inbox.enrich skip item_id=%s status=%s", item_id, item.status)
            return
        user_id = item.user_id
        url = item.url
        normalized = item.normalized_url
        kind = item.kind

    github_token = await _load_github_token(session_factory, user_id)
    gh_repo = _parse_github_repo(normalized) if kind == "github_repo" else None

    fetch_result: insight_fetch.FetchResult | None = None
    og: OpenGraph = OpenGraph()
    gh_meta: GhRepoMeta = GhRepoMeta()
    crawl_status = "success"

    try:
        fetch_result = await insight_fetch.fetch_web_content(
            url, max_chars, github_token=github_token
        )
    except Exception as exc:
        logger.warning("inbox.enrich fetch failed item_id=%s: %r", item_id, exc)
        crawl_status = "failed"

    async with httpx.AsyncClient(timeout=_OG_TIMEOUT, follow_redirects=True) as client:
        tasks: list[asyncio.Task] = []
        if kind != "github_repo":
            tasks.append(asyncio.create_task(_scrape_opengraph(client, url)))
        if gh_repo is not None:
            tasks.append(
                asyncio.create_task(
                    _fetch_github_repo(client, gh_repo[0], gh_repo[1], github_token)
                )
            )
        if tasks:
            results = await asyncio.gather(*tasks, return_exceptions=True)
            for res in results:
                if isinstance(res, OpenGraph):
                    og = res
                elif isinstance(res, GhRepoMeta):
                    gh_meta = res

    # Synthesise the enriched fields.
    title: str | None = None
    if fetch_result and fetch_result.title:
        title = fetch_result.title
    if not title and og.title:
        title = og.title
    if not title and gh_repo:
        title = f"{gh_repo[0]}/{gh_repo[1]}"

    # Inbox `summary` is the LLM-generated TL;DR (filled in by classify).
    # For now seed it with the source's own description so the card is not
    # empty before classify runs.
    summary_seed = gh_meta.description or og.description or None

    content_text = fetch_result.content if fetch_result else None

    async with session_factory() as session:
        item = await session.get(InboxItem, item_id)
        if item is None:
            return
        if title:
            item.title = title
        if summary_seed and not item.summary:
            item.summary = summary_seed
        if content_text:
            item.content_text = content_text
        if og.image and not item.og_image_url:
            item.og_image_url = og.image
        if gh_meta.owner_avatar and not item.og_image_url:
            item.og_image_url = gh_meta.owner_avatar
        if og.favicon and not item.favicon_url:
            item.favicon_url = og.favicon
        if og.author and not item.author:
            item.author = og.author
        if og.site_name and not item.publisher:
            item.publisher = og.site_name
        item.crawl_status = crawl_status
        item.status = "enriched" if crawl_status == "success" else "failed"
        await session.commit()

    logger.info(
        "inbox.enrich done item_id=%s crawl=%s via=%s",
        item_id, crawl_status, fetch_result.via if fetch_result else "none",
    )

    if arq is not None and crawl_status == "success":
        await arq.enqueue_job(
            "classify_item",
            item_id,
            _job_id=f"inbox:classify:{item_id}",
            _queue_name="inbox:default",
        )
