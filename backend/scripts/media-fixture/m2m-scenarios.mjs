import assert from 'node:assert/strict';
import { randomInt } from 'node:crypto';

export async function m2mScenarios({ request, fixtureSql, restartApp, appHttpPort, ownerToken, actor }) {
  const keys = [];
  const events = [];
  const eventType = `fixture_${randomInt(1_000_000_000)}`;
  const base = `http://127.0.0.1:${appHttpPort}/api/internal/v1`;
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  async function call(path, { key, token, method = 'GET', body, expected = 200 } = {}) {
    const headers = {};
    if (key) headers['x-api-key'] = key;
    if (token) headers.authorization = `Bearer ${token}`;
    if (body !== undefined) headers['content-type'] = 'application/json';
    const response = await fetch(base + path, {
      method, headers,
      ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
    });
    assert.equal(response.status, expected, `${method} M2M ${path}`);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    if (expected === 401) assert.notEqual(response.headers.get('www-authenticate'), 'Bearer', 'M2M must not invite JWT authentication');
    const text = await response.text();
    const data = text ? JSON.parse(text) : null;
    assert(!text.includes('key_hash'), 'M2M response must not expose key hashes');
    if (key) assert(!text.includes(key), 'M2M response must not expose the supplied secret');
    return data;
  }
  async function createKey(scopes) {
    const { data } = await request('/api/v1/api-keys', {
      token: ownerToken, method: 'POST', body: { name: `M2M fixture ${eventType}`, scopes }, expected: 201,
    });
    keys.push(data.id);
    return data;
  }
  const used = key => fixtureSql(`SELECT last_used_at FROM api_keys WHERE id=${key.id}`);
  async function status(key) {
    const data = await call('/status', { key });
    assert.deepEqual(Object.keys(data).sort(), ['active_groups', 'bot_running', 'bot_username', 'total_groups', 'total_users', 'uptime_seconds']);
    assert.equal(data.bot_running, true);
    assert.equal(data.bot_username, '@yoink_fixture_bot');
    assert.equal(data.total_users, Number(await fixtureSql('SELECT count(*) FROM users')));
    assert.equal(data.total_groups, Number(await fixtureSql('SELECT count(*) FROM groups')));
    assert.equal(data.active_groups, Number(await fixtureSql('SELECT count(*) FROM groups WHERE enabled=1')));
    assert(Number.isFinite(data.uptime_seconds) && data.uptime_seconds > 0, 'status must return actual positive uptime');
    return data;
  }
  try {
    for (const path of ['/status', '/users', `/users/${actor}`, '/groups', '/events']) {
      await call(path, { expected: 401 });
      await call(path, { token: ownerToken, expected: 401 });
      await call(path, { key: 'yoink_missing_fixture_key', expected: 401 });
    }
    const all = await createKey(['*']);
    const users = await createKey(['users:*']);
    const health = await createKey(['health:read']);
    const eventKey = await createKey(['events:*']);
    const empty = await createKey([]);
    assert.equal(await used(all), '');
    await status(all.key);
    assert(Number(await used(all)) > 0, 'successful authentication must update key usage');
    await status(health.key);
    await call('/users', { key: health.key, expected: 403 });
    await call('/status', { key: users.key, expected: 403 });
    await call('/status', { key: empty.key, expected: 403 });
    assert(Number(await used(empty)) > 0, 'valid key updates usage even when scope is denied');
    await call('/events', { key: users.key, expected: 403 });
    await call('/events', { key: users.key, method: 'POST', body: { plugin: 'fixture_m2m', event_type: eventType }, expected: 403 });
    const adminBoundary = await fetch(`http://127.0.0.1:${appHttpPort}/api/v1/api-keys`, { headers: { 'x-api-key': all.key } });
    assert.equal(adminBoundary.status, 401, 'API keys cannot enter JWT administration');

    const user = await call(`/users/${actor}`, { key: users.key });
    assert.deepEqual(Object.keys(user).sort(), ['created_at', 'first_name', 'id', 'is_blocked', 'language', 'role', 'username']);
    assert.equal(user.id, actor);
    assert.equal(user.role, await fixtureSql(`SELECT role FROM users WHERE id=${actor}`));
    assert(Number.isFinite(Date.parse(user.created_at)));
    await call('/users/9223372036854775807', { key: users.key, expected: 404 });
    const firstPage = await call('/users?limit=2', { key: users.key });
    assert.equal(firstPage.length, 2);
    assert.deepEqual(await call('/users?limit=1&offset=1', { key: users.key }), firstPage.slice(1));
    const restricted = await call('/users?role=restricted', { key: users.key });
    assert(restricted.length > 0 && restricted.every(item => item.role === 'restricted'));
    assert.deepEqual(await call('/users?role=unknown&limit=2', { key: users.key }), firstPage, 'legacy invalid role filter is ignored');
    for (const query of ['offset=-1', 'limit=0', 'limit=201']) await call(`/users?${query}`, { key: users.key, expected: 422 });
    const groups = await call('/groups', { key: all.key });
    assert.equal(groups.length, Number(await fixtureSql('SELECT count(*) FROM groups')));
    for (const value of ['true', '1', 'yes', 'on', 'TRUE']) {
      assert.deepEqual(await call(`/groups?enabled_only=${value}`, { key: all.key }), groups.filter(group => group.enabled));
    }
    for (const value of ['false', '0', 'no', 'off', 'FALSE']) {
      assert.deepEqual(await call(`/groups?enabled_only=${value}`, { key: all.key }), groups);
    }
    for (const group of groups) {
      assert.deepEqual(Object.keys(group).sort(), ['auto_grant_role', 'created_at', 'enabled', 'id', 'title']);
      assert(Number.isFinite(Date.parse(group.created_at)));
    }

    const savedRole = await fixtureSql('SELECT role FROM users WHERE id=42');
    const savedBan = await fixtureSql('SELECT ban_until FROM users WHERE id=42');
    try {
      await fixtureSql("UPDATE users SET role='banned' WHERE id=42");
      await status(all.key);
      await fixtureSql(`UPDATE users SET role=${literal(savedRole)},ban_until=(unixepoch('now')+600)*1000000 WHERE id=42`);
      await status(all.key);
    } finally {
      await fixtureSql(`UPDATE users SET role=${literal(savedRole)},ban_until=${savedBan || 'NULL'} WHERE id=42`);
    }

    const savedScopes = await fixtureSql(`SELECT scopes FROM api_keys WHERE id=${all.id}`);
    try {
      await fixtureSql(`UPDATE api_keys SET scopes='{}' WHERE id=${all.id}`);
      await call('/status', { key: all.key, expected: 503 });
      assert.equal(await fixtureSql(`SELECT scopes FROM api_keys WHERE id=${all.id}`), '{}', 'corrupt scope read must not repair data');
    } finally {
      await fixtureSql(`UPDATE api_keys SET scopes=${literal(savedScopes)} WHERE id=${all.id}`);
    }
    for (const expiry of ["unixepoch('now')*1000000", "(unixepoch('now')-60)*1000000"]) {
      await fixtureSql(`UPDATE api_keys SET expires_at=${expiry},last_used_at=NULL WHERE id=${health.id}`);
      await call('/status', { key: health.key, expected: 401 });
      assert.equal(await used(health), '', 'expired key must not update usage');
    }
    await fixtureSql(`UPDATE api_keys SET expires_at=(unixepoch('now')+600)*1000000 WHERE id=${health.id}`);
    await status(health.key);
    await request(`/api/v1/api-keys/${health.id}`, { token: ownerToken, method: 'DELETE', expected: 204 });
    const revokedUsage = await used(health);
    await call('/status', { key: health.key, expected: 401 });
    assert.equal(await used(health), revokedUsage, 'revoked key must not update usage');

    for (const body of [
      { plugin: 'fixture_m2m', event_type: eventType, payload: { label: 'Тест', nested: { n: 1 }, list: [true, null] }, user_id: actor, thread_id: 7 },
      { plugin: 'fixture_m2m', event_type: eventType, payload: null },
    ]) {
      const created = await call('/events', { key: eventKey.key, method: 'POST', body, expected: 201 });
      events.push(created.id);
      assert.deepEqual(Object.keys(created).sort(), ['created_at', 'event_type', 'group_id', 'id', 'payload', 'plugin', 'user_id']);
      assert.equal(created.plugin, body.plugin);
      assert.equal(created.event_type, body.event_type);
      assert.deepEqual(created.payload, body.payload);
      assert.equal(created.user_id, body.user_id ?? null);
      assert.equal(created.group_id, null);
      assert(Number.isFinite(Date.parse(created.created_at)));
      assert.equal(await fixtureSql(`SELECT thread_id FROM events WHERE id=${created.id}`), String(body.thread_id ?? ''));
    }
    await fixtureSql(`UPDATE events SET payload='null' WHERE id=${events[1]}`);
    const filter = `/events?plugin=fixture_m2m&event_type=${eventType}`;
    const storedEvents = await call(filter, { key: eventKey.key });
    assert.equal(storedEvents.length, 2);
    assert.equal(storedEvents.find(event => event.id === events[1]).payload, null);
    assert.equal(await fixtureSql(`SELECT payload FROM events WHERE id=${events[1]}`), 'null', 'reading imported JSON null must preserve stored representation');
    assert.deepEqual(await call(`${filter}&limit=1&offset=1`, { key: eventKey.key }), storedEvents.slice(1));
    const before = await fixtureSql('SELECT count(*) FROM events');
    for (const body of ['{', {}, { plugin: 'fixture_m2m', event_type: eventType, payload: [] }]) {
      await call('/events', { key: eventKey.key, method: 'POST', body, expected: 422 });
    }
    assert.equal(await fixtureSql('SELECT count(*) FROM events'), before, 'invalid events must not persist');
    for (const query of ['offset=-1', 'limit=0', 'limit=201']) await call(`/events?${query}`, { key: eventKey.key, expected: 422 });
    await restartApp();
    await status(all.key);
    await call('/status', { key: health.key, expected: 401 });
    assert.deepEqual(await call(filter, { key: eventKey.key }), storedEvents, 'event objects and timestamps survive restart');
    assert.deepEqual(await call(`/users/${actor}`, { key: users.key }), user, 'read-only user inspection must not rewrite profile metadata');
  } finally {
    if (events.length) await fixtureSql(`DELETE FROM events WHERE id IN (${events.join(',')})`);
    if (keys.length) await fixtureSql(`DELETE FROM api_keys WHERE id IN (${keys.join(',')})`);
  }
}
