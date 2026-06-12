"""Rules engine for the inbox plugin.

Evaluates `InboxRule` rows for a given user at the tail of:
- classify_item  (trigger='item_classified')
- ingest_url     (trigger='item_ingested')
- sync_user_stars (trigger='star_synced')

Each rule carries a list of conditions (AND-joined) and a list of actions
executed in order. Rules fire in ascending priority order (lowest int wins),
ties broken by id.

Supported conditions (field: operator: value):
  kind              eq | ne            link | github_repo | article | video | other
  status            eq | ne            pending | enriched | classified | ...
  url_contains      contains | not_contains   substring (case-insensitive)
  title_contains    contains | not_contains   substring (case-insensitive)
  category_name     eq | ne            category name (case-insensitive)
  language          eq | ne            GitHub language (star trigger only)

Supported actions (type: params):
  add_category      {"category_name": "..."} - attach category (attached_by='rule')
  set_status        {"status": "..."}        - override item status
  archive           {}                        - set status=archived + archived_at

Missing category names in add_category are created with kind='ai'. Unknown
conditions or actions are skipped with a warning so a schema migration never
breaks existing rules silently.
"""
from __future__ import annotations

import logging
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any

from sqlalchemy import select
from sqlalchemy.dialects.postgresql import insert as pg_insert

from yoink_inbox.storage.models import (
    InboxCategory,
    InboxItem,
    InboxItemCategory,
    InboxRule,
)

if TYPE_CHECKING:
    from sqlalchemy.ext.asyncio import AsyncSession

logger = logging.getLogger(__name__)

_VALID_TRIGGERS = frozenset({"item_ingested", "item_classified", "star_synced"})


# ---------------------------------------------------------------------------
# Condition evaluation
# ---------------------------------------------------------------------------


def _str_op(field_val: str | None, op: str, target: str) -> bool:
    if field_val is None:
        return op in ("ne", "not_contains")
    fv = field_val.lower()
    tv = target.lower()
    if op == "eq":
        return fv == tv
    if op == "ne":
        return fv != tv
    if op == "contains":
        return tv in fv
    if op == "not_contains":
        return tv not in fv
    logger.warning("inbox.rules unknown string op=%r, skipping", op)
    return True


def _eval_condition(cond: dict[str, Any], item: InboxItem, category_names: list[str]) -> bool:
    field: str = cond.get("field", "")
    op: str = cond.get("op", "eq")
    value: Any = cond.get("value", "")

    if field == "kind":
        return _str_op(item.kind, op, str(value))
    if field == "status":
        return _str_op(item.status, op, str(value))
    if field == "url_contains":
        return _str_op(item.url, op if op in ("contains", "not_contains") else "contains", str(value))
    if field == "title_contains":
        return _str_op(item.title, op if op in ("contains", "not_contains") else "contains", str(value))
    if field == "category_name":
        lowered = [c.lower() for c in category_names]
        target = str(value).lower()
        if op == "eq":
            return target in lowered
        if op == "ne":
            return target not in lowered
        logger.warning("inbox.rules category_name unsupported op=%r", op)
        return True
    # star_synced trigger fields -- not evaluated on InboxItem
    if field == "language":
        return True

    logger.warning("inbox.rules unknown condition field=%r, treating as True", field)
    return True


def _matches(rule: InboxRule, item: InboxItem, category_names: list[str]) -> bool:
    """Return True if all conditions in the rule pass (AND semantics)."""
    for cond in (rule.conditions or []):
        if not isinstance(cond, dict):
            continue
        if not _eval_condition(cond, item, category_names):
            return False
    return True


def evaluate_rule_conditions(
    rule: InboxRule, item: InboxItem, category_names: list[str]
) -> list[dict]:
    """Return per-condition results for the test endpoint."""
    results = []
    for cond in (rule.conditions or []):
        if not isinstance(cond, dict):
            continue
        passed = _eval_condition(cond, item, category_names)
        results.append({**cond, "passed": passed})
    return results


# ---------------------------------------------------------------------------
# Action execution
# ---------------------------------------------------------------------------


async def _exec_action(
    session: "AsyncSession",
    action: dict[str, Any],
    item: InboxItem,
    user_id: int,
) -> None:
    action_type: str = action.get("type", "")

    if action_type == "add_category":
        cat_name: str = str(action.get("params", {}).get("category_name", "")).strip()
        if not cat_name:
            logger.warning("inbox.rules add_category missing category_name")
            return
        cat = await _get_or_create_category(session, user_id, cat_name)
        if cat is None:
            return
        stmt = pg_insert(InboxItemCategory).values(
            item_id=item.id,
            category_id=cat.id,
            attached_by="rule",
            confidence=None,
        )
        stmt = stmt.on_conflict_do_nothing(index_elements=["item_id", "category_id"])
        await session.execute(stmt)
        logger.info(
            "inbox.rules add_category item_id=%s cat=%r", item.id, cat_name
        )

    elif action_type == "set_status":
        new_status: str = str(action.get("params", {}).get("status", "")).strip()
        if not new_status:
            logger.warning("inbox.rules set_status missing status param")
            return
        item.status = new_status
        logger.info("inbox.rules set_status item_id=%s status=%r", item.id, new_status)

    elif action_type == "archive":
        item.status = "archived"
        item.archived_at = datetime.now(timezone.utc)
        logger.info("inbox.rules archive item_id=%s", item.id)

    else:
        logger.warning("inbox.rules unknown action type=%r, skipping", action_type)


async def _get_or_create_category(
    session: "AsyncSession", user_id: int, name: str
) -> InboxCategory | None:
    from re import sub as re_sub

    def _norm(n: str) -> str:
        return re_sub(r"[ _-]", "", n).lower()

    norm = _norm(name)
    existing = (
        await session.scalars(
            select(InboxCategory).where(InboxCategory.owner_user_id == user_id)
        )
    ).all()
    by_norm = {_norm(c.name): c for c in existing}
    if norm in by_norm:
        return by_norm[norm]

    slug = (norm or "uncategorised")[:64]
    cat = InboxCategory(
        owner_user_id=user_id,
        name=name,
        slug=slug,
        kind="ai",
    )
    session.add(cat)
    try:
        await session.flush()
    except Exception:
        await session.rollback()
        logger.exception(
            "inbox.rules failed to create category name=%r user_id=%s", name, user_id
        )
        return None
    return cat


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


async def run_rules(
    session: "AsyncSession",
    *,
    user_id: int,
    item_id: int,
    trigger: str,
) -> int:
    """Evaluate and execute all matching rules for `item_id`.

    Called inside an open session; caller commits after this returns.
    Returns the count of rules that fired.
    """
    if trigger not in _VALID_TRIGGERS:
        logger.warning("inbox.rules unknown trigger=%r", trigger)
        return 0

    item = await session.get(InboxItem, item_id)
    if item is None or item.user_id != user_id:
        logger.warning("inbox.rules item_id=%s not found for user_id=%s", item_id, user_id)
        return 0

    rules = (
        await session.scalars(
            select(InboxRule)
            .where(
                InboxRule.user_id == user_id,
                InboxRule.enabled == True,  # noqa: E712
                InboxRule.trigger == trigger,
            )
            .order_by(InboxRule.priority.asc(), InboxRule.id.asc())
        )
    ).all()

    if not rules:
        return 0

    # Load current category names for condition evaluation.
    cat_rows = (
        await session.scalars(
            select(InboxItemCategory)
            .where(InboxItemCategory.item_id == item_id)
        )
    ).all()
    cat_ids = [r.category_id for r in cat_rows]
    cat_names: list[str] = []
    if cat_ids:
        cats = (
            await session.scalars(
                select(InboxCategory).where(InboxCategory.id.in_(cat_ids))
            )
        ).all()
        cat_names = [c.name for c in cats]

    fired = 0
    for rule in rules:
        if not _matches(rule, item, cat_names):
            continue
        logger.info(
            "inbox.rules FIRE rule_id=%s %r item_id=%s trigger=%s",
            rule.id, rule.name, item_id, trigger,
        )
        for action in (rule.actions or []):
            if not isinstance(action, dict):
                continue
            await _exec_action(session, action, item, user_id)
        fired += 1

    return fired
