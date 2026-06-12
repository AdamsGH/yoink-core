"""Tests for the inbox rules engine and REST CRUD.

Service tests (tests 6-7) call apply_rules / run_rules directly.
API tests (tests 1-5, 8) use a minimal FastAPI app that mounts only the
inbox router, wired to the test DB via the same session_factory fixture
used by the rest of the suite.
"""
from __future__ import annotations

import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from sqlalchemy import select
from yoink_inbox.api.router import router as inbox_router
from yoink_inbox.services.rules import run_rules
from yoink_inbox.storage.models import (
    InboxCategory,
    InboxItem,
    InboxItemCategory,
    InboxRule,
)

from yoink.core.api.deps import get_current_user

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

RULE_PAYLOAD = {
    "name": "tag articles",
    "enabled": True,
    "priority": 10,
    "trigger": "item_classified",
    "conditions": [{"field": "kind", "op": "eq", "value": "link"}],
    "actions": [{"type": "set_status", "params": {"status": "flagged"}}],
}


@pytest_asyncio.fixture
async def inbox_client(session_factory, owner):
    """Bare FastAPI with inbox router; no lifespan so the test engine is not replaced."""
    from fastapi import FastAPI

    app = FastAPI()
    # _require_feature and get_db both read from app.state.session_factory.
    app.state.session_factory = session_factory
    app.state.bot_data = {}
    app.state.bot = None

    async def _get_user():
        return owner

    app.dependency_overrides[get_current_user] = _get_user
    app.include_router(inbox_router, prefix="/inbox")

    transport = ASGITransport(app=app)
    async with AsyncClient(transport=transport, base_url="http://test") as client:
        yield client


@pytest_asyncio.fixture
async def inbox_item(session_factory, owner):
    """A bare InboxItem owned by `owner` for use in service-layer tests."""
    async with session_factory() as sess:
        item = InboxItem(
            user_id=owner.id,
            url="https://example.com/article",
            normalized_url="https://example.com/article",
            kind="link",
            status="classified",
        )
        sess.add(item)
        await sess.commit()
        await sess.refresh(item)
    yield item
    async with session_factory() as sess:
        obj = await sess.get(InboxItem, item.id)
        if obj:
            await sess.delete(obj)
            await sess.commit()


@pytest_asyncio.fixture
async def inbox_category(session_factory, owner):
    """A bare InboxCategory owned by `owner`."""
    async with session_factory() as sess:
        cat = InboxCategory(owner_user_id=owner.id, name="Articles", slug="articles")
        sess.add(cat)
        await sess.commit()
        await sess.refresh(cat)
    yield cat
    async with session_factory() as sess:
        obj = await sess.get(InboxCategory, cat.id)
        if obj:
            await sess.delete(obj)
            await sess.commit()


# ---------------------------------------------------------------------------
# Test 1 - POST /rules creates a rule
# ---------------------------------------------------------------------------

async def test_create_rule(inbox_client):
    r = await inbox_client.post("/inbox/rules", json=RULE_PAYLOAD)
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["name"] == RULE_PAYLOAD["name"]
    assert body["trigger"] == RULE_PAYLOAD["trigger"]
    assert body["priority"] == 10
    assert body["enabled"] is True
    assert isinstance(body["id"], int)


# ---------------------------------------------------------------------------
# Test 2 - GET /rules lists the created rule
# ---------------------------------------------------------------------------

async def test_list_rules(inbox_client):
    await inbox_client.post("/inbox/rules", json=RULE_PAYLOAD)
    r = await inbox_client.get("/inbox/rules")
    assert r.status_code == 200
    rules = r.json()
    assert any(rule["name"] == RULE_PAYLOAD["name"] for rule in rules)


# ---------------------------------------------------------------------------
# Test 3 - PUT /rules/{id} replaces all fields
# ---------------------------------------------------------------------------

async def test_replace_rule(inbox_client):
    cr = await inbox_client.post("/inbox/rules", json=RULE_PAYLOAD)
    rule_id = cr.json()["id"]

    replacement = {
        "name": "renamed rule",
        "enabled": False,
        "priority": 50,
        "trigger": "item_ingested",
        "conditions": [],
        "actions": [],
    }
    r = await inbox_client.put(f"/inbox/rules/{rule_id}", json=replacement)
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["name"] == "renamed rule"
    assert body["enabled"] is False
    assert body["priority"] == 50
    assert body["trigger"] == "item_ingested"
    assert body["conditions"] == []
    assert body["actions"] == []


# ---------------------------------------------------------------------------
# Test 4 - PATCH /rules/{id} updates only enabled, leaves other fields intact
# ---------------------------------------------------------------------------

async def test_patch_rule_enabled_only(inbox_client):
    cr = await inbox_client.post("/inbox/rules", json=RULE_PAYLOAD)
    rule_id = cr.json()["id"]

    r = await inbox_client.patch(f"/inbox/rules/{rule_id}", json={"enabled": False})
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["enabled"] is False
    # Other fields unchanged
    assert body["name"] == RULE_PAYLOAD["name"]
    assert body["priority"] == RULE_PAYLOAD["priority"]
    assert body["trigger"] == RULE_PAYLOAD["trigger"]


# ---------------------------------------------------------------------------
# Test 5 - DELETE /rules/{id} returns 204; subsequent GET returns 404
# ---------------------------------------------------------------------------

async def test_delete_rule(inbox_client):
    cr = await inbox_client.post("/inbox/rules", json=RULE_PAYLOAD)
    rule_id = cr.json()["id"]

    r = await inbox_client.delete(f"/inbox/rules/{rule_id}")
    assert r.status_code == 204, r.text

    r2 = await inbox_client.get(f"/inbox/rules/{rule_id}")
    assert r2.status_code == 404


# ---------------------------------------------------------------------------
# Test 6 - run_rules fires set_category action when conditions match
# ---------------------------------------------------------------------------

async def test_run_rules_fires_on_match(session_factory, owner, inbox_item, inbox_category):
    async with session_factory() as sess:
        rule = InboxRule(
            user_id=owner.id,
            name="auto-tag links",
            enabled=True,
            priority=10,
            trigger="item_classified",
            conditions=[{"field": "kind", "op": "eq", "value": "link"}],
            actions=[{"type": "add_category", "params": {"category_name": inbox_category.name}}],
        )
        sess.add(rule)
        await sess.commit()
        rule_id = rule.id

    async with session_factory() as sess:
        fired = await run_rules(
            sess,
            user_id=owner.id,
            trigger="item_classified",
            item_id=inbox_item.id,
        )
        await sess.commit()

    assert fired >= 1

    # Verify the InboxItemCategory row was created.
    async with session_factory() as sess:
        row = await sess.scalar(
            select(InboxItemCategory).where(
                InboxItemCategory.item_id == inbox_item.id,
                InboxItemCategory.category_id == inbox_category.id,
            )
        )
    assert row is not None

    # Cleanup
    async with session_factory() as sess:
        r = await sess.get(InboxRule, rule_id)
        if r:
            await sess.delete(r)
            await sess.commit()


# ---------------------------------------------------------------------------
# Test 7 - run_rules does NOT fire when conditions do not match
# ---------------------------------------------------------------------------

async def test_run_rules_no_fire_on_mismatch(session_factory, owner, inbox_item):
    async with session_factory() as sess:
        rule = InboxRule(
            user_id=owner.id,
            name="video-only rule",
            enabled=True,
            priority=10,
            trigger="item_classified",
            # Condition will NOT match: inbox_item.kind == "link", not "video"
            conditions=[{"field": "kind", "op": "eq", "value": "video"}],
            actions=[{"type": "set_status", "params": {"status": "archived"}}],
        )
        sess.add(rule)
        await sess.commit()
        rule_id = rule.id

    async with session_factory() as sess:
        fired = await run_rules(
            sess,
            user_id=owner.id,
            trigger="item_classified",
            item_id=inbox_item.id,
        )
        await sess.commit()

    assert fired == 0

    # Status must be unchanged
    async with session_factory() as sess:
        item = await sess.get(InboxItem, inbox_item.id)
    assert item.status == "classified"

    # Cleanup
    async with session_factory() as sess:
        r = await sess.get(InboxRule, rule_id)
        if r:
            await sess.delete(r)
            await sess.commit()


# ---------------------------------------------------------------------------
# Test 8 - POST /rules/{id}/test returns matched + conditions_result
# ---------------------------------------------------------------------------

async def test_rule_test_endpoint(inbox_client, inbox_item):
    cr = await inbox_client.post("/inbox/rules", json={
        **RULE_PAYLOAD,
        "conditions": [{"field": "kind", "op": "eq", "value": "link"}],
    })
    rule_id = cr.json()["id"]

    r = await inbox_client.post(
        f"/inbox/rules/{rule_id}/test",
        params={"item_id": inbox_item.id},
    )
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["rule_id"] == rule_id
    assert body["matched"] is True
    assert len(body["conditions_result"]) == 1
    assert body["conditions_result"][0]["passed"] is True
    assert len(body["actions_would_fire"]) >= 1
