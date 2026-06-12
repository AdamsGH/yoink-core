"""Hybrid LLM categorisation for inbox items.

Strategy:
1. Load the (enriched) item + the user's existing category catalogue.
2. Build the hybrid prompt: model sees existing names + may propose AT MOST
   ONE new category marked `kind=new`.
3. Call `yoink_insight.services.llm.complete` (BYOK or gateway, the insight
   plugin owns that routing).
4. Parse JSON with a regex salvage fallback (LLMs sometimes wrap the body in
   prose or in a fenced code block).
5. Apply categories via `_resolve_categories` which dedups on the
   `normalized_name` generated column and creates missing rows with
   `kind=ai`. Bindings go through `inbox_item_categories` with
   `attached_by='ai'`.
6. Item status -> classified, llm_status -> success/failed.

Failure modes never block ingest: any exception leaves status alone (still
`enriched`) and llm_status='failed' so the operator can retry.
"""
from __future__ import annotations

import json
import logging
import re
from dataclasses import dataclass
from typing import TYPE_CHECKING

from sqlalchemy import select
from sqlalchemy.dialects.postgresql import insert as pg_insert

from yoink_inbox.storage.models import (
    InboxCategory,
    InboxItem,
    InboxItemCategory,
)

if TYPE_CHECKING:
    from sqlalchemy.ext.asyncio import async_sessionmaker

logger = logging.getLogger(__name__)


# Per-item content budget in characters. tiktoken-precise truncation is not
# warranted here; the prompt is short and the content excerpt dominates.
# Keep this below `InboxConfig.inbox_classify_token_budget * ~4 chars/token`.
_CONTENT_BUDGET_CHARS = 8000

# Hard caps on what the model can do, regardless of `inbox_classify_*` config.
_MAX_NEW_CATEGORIES_PER_CALL = 1
_MAX_CATEGORIES_PER_ITEM = 3

# Used to dedup category names against the Postgres generated `normalized_name`
# column (lower + strip space/-/_).
_NORMALIZE_RE = re.compile(r"[ _-]")

# Extract the first {...} block from a response. Greedy on the outer braces;
# good enough for the rare \"prefix prose + JSON + suffix\" case.
_JSON_BLOCK_RE = re.compile(r"\{.*\}", re.DOTALL)


def _normalize(name: str) -> str:
    return _NORMALIZE_RE.sub("", name).lower()


@dataclass(slots=True)
class _CategoryProposal:
    name: str
    kind: str  # "existing" | "new"
    confidence: float | None


@dataclass(slots=True)
class _ParsedResponse:
    categories: list[_CategoryProposal]
    summary: str | None
    is_github_repo: bool | None


def _parse_response(raw: str) -> _ParsedResponse:
    """Parse the LLM output into a typed structure.

    Tries strict JSON first, then salvages the first balanced `{...}` blob.
    Returns an empty categories list rather than raising; the caller treats
    that as a valid \"no labels apply\" outcome.
    """
    payload: dict | None = None
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        m = _JSON_BLOCK_RE.search(raw)
        if m is not None:
            try:
                payload = json.loads(m.group(0))
            except json.JSONDecodeError:
                payload = None

    if not isinstance(payload, dict):
        logger.warning("inbox.classify could not parse JSON from LLM, len=%d", len(raw))
        return _ParsedResponse(categories=[], summary=None, is_github_repo=None)

    cats: list[_CategoryProposal] = []
    for entry in payload.get("categories") or []:
        if not isinstance(entry, dict):
            continue
        name = (entry.get("name") or "").strip()
        if not name:
            continue
        kind = entry.get("kind") or "existing"
        if kind not in {"existing", "new"}:
            kind = "existing"
        conf_raw = entry.get("confidence")
        conf: float | None
        try:
            conf = float(conf_raw) if conf_raw is not None else None
        except (TypeError, ValueError):
            conf = None
        cats.append(_CategoryProposal(name=name, kind=kind, confidence=conf))
        if len(cats) >= _MAX_CATEGORIES_PER_ITEM:
            break

    summary_raw = payload.get("summary")
    summary = summary_raw.strip() if isinstance(summary_raw, str) and summary_raw.strip() else None

    gh_raw = payload.get("is_github_repo")
    is_gh = gh_raw if isinstance(gh_raw, bool) else None

    return _ParsedResponse(categories=cats, summary=summary, is_github_repo=is_gh)


_DEFAULT_SYSTEM_PROMPT = (
    "You are categorising an item for a personal inbox. "
    "Be concise and accurate."
)


def _build_prompt(
    title: str | None,
    url: str,
    content: str | None,
    existing_categories: list[str],
    *,
    allow_new: bool,
    system_prompt_override: str | None = None,
    user_hint: str | None = None,
    response_language: str = "en",
) -> str:
    """Assemble the classification prompt.

    `system_prompt_override` replaces the default system line when set (admin).
    `user_hint` is appended after the main instruction (per-user personalisation).
    `content` is truncated to `_CONTENT_BUDGET_CHARS`.
    """
    if content and len(content) > _CONTENT_BUDGET_CHARS:
        content = content[:_CONTENT_BUDGET_CHARS] + "\n\n[truncated]"

    system_line = system_prompt_override or _DEFAULT_SYSTEM_PROMPT
    existing_block = "\n".join(f"- {n}" for n in existing_categories) or "(none yet)"
    new_clause = (
        f"You MAY propose AT MOST {_MAX_NEW_CATEGORIES_PER_CALL} new category "
        f"if nothing existing fits well; mark it with kind=new."
        if allow_new
        else "You MUST pick from the existing categories only; never invent new ones."
    )
    hint_block = f"\nUser context: {user_hint.strip()}" if user_hint and user_hint.strip() else ""
    lang_instruction = (
        f"Write all category names and the summary in {response_language.upper()} language."
        if response_language != "en"
        else ""
    )
    lang_block = f"\n{lang_instruction}" if lang_instruction else ""

    return f"""{system_line}

The user already has these categories:
{existing_block}

Pick UP TO {_MAX_CATEGORIES_PER_ITEM} categories that fit the item. {new_clause}{hint_block}{lang_block}

Output ONLY JSON with this exact shape:
{{
  "categories": [
    {{"name": "...", "kind": "existing" or "new", "confidence": 0.0-1.0}}
  ],
  "summary": "one to three sentences describing the item",
  "is_github_repo": true or false
}}

TITLE: {title or "(none)"}
URL:   {url}
EXTRACTED_CONTENT:
{content or "(no extracted content)"}
"""


async def _existing_category_names(
    session, user_id: int,
) -> list[InboxCategory]:
    """All categories the user owns OR can write into via a team share.

    Team-shared categories are visible AND writable by team members per the
    Q3 decision in TODO-c66b1c02. For phase-1 we surface only the names; a
    later commit can show owner/team membership on hover.
    """
    # Simple cut for now: own categories. Team-shared categories arrive
    # alongside the teams CRUD endpoint; loading them here would force a
    # subquery against `inbox_team_members` that has no rows yet. Wire up in
    # the same commit that introduces /teams REST.
    rows = (
        await session.scalars(
            select(InboxCategory).where(InboxCategory.owner_user_id == user_id)
        )
    ).all()
    return list(rows)


async def _resolve_categories(
    session,
    user_id: int,
    proposals: list[_CategoryProposal],
    existing: list[InboxCategory],
) -> list[tuple[InboxCategory, float | None]]:
    """Map proposals onto persisted InboxCategory rows.

    - For `kind=existing` we look up by normalised name among the user's
      existing categories. A miss is logged and the proposal is dropped (the
      model hallucinated a label we never sent).
    - For `kind=new` we insert a row with `kind='ai'`. If the normalised name
      collides with an existing category we treat the proposal as that
      category instead (LLM relabelling something we already had).
    - At most `_MAX_NEW_CATEGORIES_PER_CALL` `new` proposals are honoured;
      extras are downgraded to dropped.
    """
    by_normalised: dict[str, InboxCategory] = {
        _normalize(c.name): c for c in existing
    }
    resolved: list[tuple[InboxCategory, float | None]] = []
    new_used = 0

    for prop in proposals:
        norm = _normalize(prop.name)
        hit = by_normalised.get(norm)
        if hit is not None:
            resolved.append((hit, prop.confidence))
            continue

        if prop.kind != "new":
            logger.info(
                "inbox.classify dropped hallucinated existing category=%r user_id=%s",
                prop.name, user_id,
            )
            continue
        if new_used >= _MAX_NEW_CATEGORIES_PER_CALL:
            continue

        # Create a new category. Slug is the normalised name truncated to 64
        # chars; collisions are unlikely because (owner_user_id, slug) is
        # unique and we already deduped on normalized_name above.
        slug = (norm or "uncategorised")[:64]
        row = InboxCategory(
            owner_user_id=user_id,
            name=prop.name.strip(),
            slug=slug,
            kind="ai",
        )
        session.add(row)
        try:
            await session.flush()
        except Exception:
            await session.rollback()
            logger.exception(
                "inbox.classify failed to insert new category name=%r user_id=%s",
                prop.name, user_id,
            )
            continue
        by_normalised[norm] = row
        resolved.append((row, prop.confidence))
        new_used += 1

    return resolved


async def _notify_classified(
    session_factory: "async_sessionmaker",
    item_id: int,
    user_id: int,
    *,
    summary: str | None,
    categories: list[str],
    response_language: str,
) -> None:
    """Edit the placeholder bot message with the classification result.

    `save.py` stores `tg_chat_id` / `tg_reply_message_id` on the item so
    the worker can reach back and replace the placeholder text.  Uses
    `python-telegram-bot.Bot` directly (no PTB Application needed in the
    worker process).
    """
    try:
        from yoink.core.config import CoreSettings  # noqa: PLC0415
        from yoink_inbox.storage.models import InboxItem  # noqa: PLC0415
        from telegram import Bot  # noqa: PLC0415

        async with session_factory() as session:
            item = await session.get(InboxItem, item_id)
            if item is None or item.tg_chat_id is None or item.tg_reply_message_id is None:
                return
            chat_id = item.tg_chat_id
            message_id = item.tg_reply_message_id
            title = item.title or item.url

        # Build the final text
        cat_tags = " ".join(f"#{c.replace(' ', '_')}" for c in categories) if categories else ""
        lines: list[str] = []
        if summary:
            lines.append(summary)
        if cat_tags:
            lines.append(cat_tags)
        body = "\n\n".join(lines) if lines else title

        cfg = CoreSettings()
        bot = Bot(token=cfg.bot_token)
        async with bot:
            await bot.edit_message_text(
                chat_id=chat_id,
                message_id=message_id,
                text=body,
            )
        logger.info("inbox.classify notify edited message item_id=%s", item_id)
    except Exception:
        logger.debug("inbox.classify notify failed item_id=%s", item_id, exc_info=True)


async def run_classify(
    session_factory: "async_sessionmaker", item_id: int
) -> None:
    """End-to-end classify pass on one inbox item.

    Idempotent: existing AI bindings for the item are wiped and replaced
    each run, so a manual `/items/{id}/reclassify` cleanly overrides earlier
    labels. User-attached bindings (attached_by='user') are preserved.
    """
    from yoink_insight.services.llm import LlmCompletionError, complete

    async with session_factory() as session:
        item = await session.get(InboxItem, item_id)
        if item is None:
            logger.warning("inbox.classify missing item_id=%s", item_id)
            return
        if item.status in {"archived", "failed"}:
            logger.info("inbox.classify skip item_id=%s status=%s", item_id, item.status)
            return
        user_id = item.user_id
        title = item.title
        url = item.url
        content = item.content_text

        existing = await _existing_category_names(session, user_id)
        existing_names = [c.name for c in existing]

        # Load admin system prompt override, user hint and AI language
        from yoink_inbox.storage.models import InboxAdminSettings, InboxUserSettings  # noqa: PLC0415
        admin_row = await session.get(InboxAdminSettings, "classify_system_prompt")
        user_row = await session.get(InboxUserSettings, user_id)
        system_override = admin_row.value if admin_row else None
        user_hint = user_row.classify_user_hint if user_row else None

        # Prefer insight_access.lang as the AI response language
        response_language = "en"
        try:
            from yoink_insight.storage.models import InsightAccess  # noqa: PLC0415
            access_row = await session.get(InsightAccess, user_id)
            if access_row and access_row.lang:
                response_language = access_row.lang
        except ImportError:
            pass

    prompt = _build_prompt(
        title, url, content, existing_names,
        allow_new=True,
        system_prompt_override=system_override,
        user_hint=user_hint,
        response_language=response_language,
    )

    try:
        raw = await complete(session_factory, user_id, prompt)
    except LlmCompletionError as exc:
        logger.warning("inbox.classify LLM failed item_id=%s code=%s", item_id, exc)
        async with session_factory() as s:
            item = await s.get(InboxItem, item_id)
            if item is not None:
                item.llm_status = "failed"
                await s.commit()
        return

    parsed = _parse_response(raw)
    logger.info(
        "inbox.classify parsed item_id=%s cats=%d summary=%s gh=%s",
        item_id, len(parsed.categories),
        "yes" if parsed.summary else "no", parsed.is_github_repo,
    )

    async with session_factory() as session:
        item = await session.get(InboxItem, item_id)
        if item is None:
            return

        # Refresh existing list because the user may have edited categories
        # between the prompt build and the write commit. Avoids stale-snapshot
        # category dedup races.
        existing = await _existing_category_names(session, user_id)

        # Wipe AI bindings; user bindings keep.
        for binding in (
            await session.scalars(
                select(InboxItemCategory).where(
                    InboxItemCategory.item_id == item_id,
                    InboxItemCategory.attached_by == "ai",
                )
            )
        ).all():
            await session.delete(binding)

        resolved = await _resolve_categories(
            session, user_id, parsed.categories, existing,
        )

        for category, confidence in resolved:
            stmt = pg_insert(InboxItemCategory).values(
                item_id=item_id,
                category_id=category.id,
                attached_by="ai",
                confidence=confidence,
            )
            stmt = stmt.on_conflict_do_nothing(
                index_elements=["item_id", "category_id"]
            )
            await session.execute(stmt)

        if parsed.summary:
            item.summary = parsed.summary

        item.status = "classified"
        item.llm_status = "success"

        from yoink_inbox.services.rules import run_rules

        await run_rules(session, user_id=user_id, item_id=item_id, trigger="item_classified")
        await session.commit()

    logger.info(
        "inbox.classify wrote item_id=%s status=classified cats=%d",
        item_id, len(resolved),
    )

    # Notify the user via Telegram with a short summary
    await _notify_classified(
        session_factory, item_id, user_id,
        summary=parsed.summary,
        categories=[cat.name for cat, _ in resolved],
        response_language=response_language,
    )
