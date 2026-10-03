import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';

export async function apiKeyScenarios({ request, fixtureSql, ownerToken, adminToken, ordinaryToken }) {
  const path = '/api/v1/api-keys';
  const created = [];
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  async function list(token = ownerToken) {
    const { data } = await request(path, { token, expected: 200 });
    assert(Array.isArray(data));
    for (const row of data) {
      assert(!Object.hasOwn(row, 'key'), 'list must never return raw keys');
      assert(!Object.hasOwn(row, 'key_hash'), 'list must never return stored hashes');
    }
    return data;
  }
  const cleanup = async () => {
    for (const key of created) await fixtureSql(`DELETE FROM api_keys WHERE id=${key.id}`);
  };
  try {
    await request(path, { expected: 401 });
    await request(`${path}/scopes`, { token: ordinaryToken, expected: 403 });
    await request(path, { token: ordinaryToken, expected: 403 });
    await request(path, { token: ordinaryToken, method: 'POST', body: { name: 'denied' }, expected: 403 });
    const scopes = (await request(`${path}/scopes`, { token: adminToken, expected: 200 })).data.scopes;
    assert.deepEqual(scopes, ['users:read', 'users:write', 'groups:read', 'groups:write', 'downloads:read', 'settings:read', 'settings:write', 'events:read', 'events:write', 'health:read']);
    async function create(token, body) {
      const { data } = await request(path, { token, method: 'POST', body, expected: 201 });
      assert(Number.isSafeInteger(data.id) && data.id > 0);
      created.push(data);
      assert.match(data.key, /^yoink_[A-Za-z0-9_-]{43}$/);
      assert.equal(data.prefix, data.key.slice(0, 8));
      assert(!Object.hasOwn(data, 'key_hash'));
      assert.equal(await fixtureSql(`SELECT key_hash FROM api_keys WHERE id=${data.id}`), createHash('sha256').update(data.key).digest('hex'));
      assert.notEqual(await fixtureSql(`SELECT key_hash FROM api_keys WHERE id=${data.id}`), data.key);
      return data;
    }
    const expires = '2037-01-02T03:04:05.123456Z';
    const first = await create(adminToken, { name: 'Ключ fixture', scopes: ['users:read', 'groups:*'], expires_at: expires });
    assert.equal(first.name, 'Ключ fixture');
    assert.deepEqual(first.scopes, ['users:read', 'groups:*']);
    assert.equal(first.expires_at, expires);
    const second = await create(ownerToken, { name: 'fixture default' });
    assert.deepEqual(second.scopes, ['*']);
    assert.equal(second.expires_at, null);
    assert.notEqual(first.key, second.key, 'keys must have independent random entropy');
    const rows = await list(adminToken);
    const listed = rows.find(row => row.id === first.id);
    assert(listed, 'admins can list keys from the shared store');
    assert.equal(listed.name, first.name);
    assert.equal(listed.last_used_at, null);
    assert.equal(listed.revoked, false);
    assert.equal(typeof listed.created_by, 'number');
    assert.deepEqual(listed.scopes, first.scopes);
    const count = await fixtureSql('SELECT count(*) FROM api_keys');
    for (const [body, expected] of [
      ['{', 422],
      [{ name: 'fixture invalid', scopes: ['unknown:read'] }, 400],
      [{ name: 'fixture invalid', scopes: 'users:read' }, 422],
      [{ name: 'fixture invalid', expires_at: 'not-a-date' }, 422],
    ]) {
      await request(path, { token: ownerToken, method: 'POST', body, expected });
      assert.equal(await fixtureSql('SELECT count(*) FROM api_keys'), count, 'invalid creation must preserve existing keys');
    }
    await request(`${path}/${first.id}`, { token: ordinaryToken, method: 'DELETE', expected: 403 });
    await request(`${path}/${second.id}`, { token: adminToken, method: 'DELETE', expected: 204 });
    await request(`${path}/${second.id}`, { token: ownerToken, method: 'DELETE', expected: 204 });
    assert.equal((await list()).find(row => row.id === second.id).revoked, true);
    await request(`${path}/9007199254740991`, { token: ownerToken, method: 'DELETE', expected: 404 });
    const savedScopes = await fixtureSql(`SELECT scopes FROM api_keys WHERE id=${first.id}`);
    try {
      await fixtureSql(`UPDATE api_keys SET scopes='{}' WHERE id=${first.id}`);
      await request(path, { token: ownerToken, expected: 503 });
      assert.equal(await fixtureSql(`SELECT scopes FROM api_keys WHERE id=${first.id}`), '{}', 'reads must not repair corrupt imported scopes');
    } finally {
      await fixtureSql(`UPDATE api_keys SET scopes=${literal(savedScopes)} WHERE id=${first.id}`);
    }
    return {
      async assertDenied(token) {
        await request(path, { token, expected: 403 });
        await request(`${path}/scopes`, { token, expected: 403 });
        await request(path, { token, method: 'POST', body: { name: 'denied' }, expected: 403 });
        await request(`${path}/${first.id}`, { token, method: 'DELETE', expected: 403 });
        assert.equal(await fixtureSql('SELECT count(*) FROM api_keys'), count);
        assert.equal(await fixtureSql(`SELECT revoked FROM api_keys WHERE id=${first.id}`), '0');
      },
      async assertPersisted() {
        const persisted = await list();
        assert.deepEqual(persisted.find(row => row.id === first.id), listed, 'active key metadata must survive restart');
        assert.equal(persisted.find(row => row.id === second.id).revoked, true, 'revocation survives restart');
        await request(`${path}/${first.id}`, { token: ownerToken, method: 'DELETE', expected: 204 });
        assert.equal((await list()).find(row => row.id === first.id).revoked, true);
      },
      cleanup,
    };
  } catch (error) {
    await cleanup();
    throw error;
  }
}
