import assert from 'node:assert/strict';
import { createHmac, randomInt } from 'node:crypto';
import { commandHarness } from './command-harness.mjs';
import { apiKeyScenarios } from './api-key-scenarios.mjs';

export async function runAdminScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, sourcePort, appHttpPort, authSecret } = context;
  assert.equal(typeof authSecret, 'string');
  assert(authSecret.length >= 32, 'fixture JWT secret must satisfy the native authentication minimum');
  const h = commandHarness(context, 1_700_000);
  const registeredActor = 7_000_000_000_000 + randomInt(1_000_000_000);
  const telegramActor = registeredActor + 1;
  let messageId = 1_800_000;
  let apiKeys;
  const base = `http://127.0.0.1:${appHttpPort}`;
  const literal = value => `'${String(value).replaceAll("'", "''")}'`;
  const savedPolicy = await fixtureSql("SELECT value FROM bot_settings WHERE key='bot_access_mode'");
  const savedTagMap = await fixtureSql("SELECT value FROM bot_settings WHERE key='tag_map'");
  const operations = () => fixtureSql('SELECT count(*) FROM runtime_operations');
  const signedInitData = id => {
    const authDate = String(Math.floor(Date.now() / 1000));
    const user = JSON.stringify({ id, first_name: 'Fixture Admin Actor', username: `fixture_${id}` });
    const key = createHmac('sha256', 'WebAppData').update(context.telegramToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${authDate}\nuser=${user}`).digest('hex');
    return new URLSearchParams({ auth_date: authDate, user, hash }).toString();
  };
  async function request(path, { token, method = 'GET', body, expected, unauthenticated = false } = {}) {
    const headers = {};
    if (token) headers.authorization = `Bearer ${token}`;
    if (body !== undefined) headers['content-type'] = 'application/json';
    const response = await fetch(base + path, {
      method,
      headers,
      ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
    });
    assert.equal(response.headers.get('cache-control'), 'no-store', `${method} ${path} must be non-cacheable`);
    if (expected !== undefined) assert.equal(response.status, expected, `${method} ${path}`);
    if (response.status === 401) assert.equal(response.headers.get('www-authenticate'), 'Bearer');
    if (unauthenticated) assert.equal(response.status, 401);
    const text = await response.text();
    let data;
    try { data = text ? JSON.parse(text) : null; } catch { data = text; }
    return { response, data };
  }
  async function login(id) {
    const result = await request('/api/v1/auth/token', {
      method: 'POST', body: { init_data: signedInitData(id) }, expected: 200,
    });
    assert.equal(result.data.token_type, 'bearer');
    assert.equal(typeof result.data.access_token, 'string');
    return result.data.access_token;
  }
  async function send(actor, command, language = 'en') {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, command, messageId++, language);
    return waitFor(() => botApi.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === actor),
      `admin fixture Telegram ${actor} ${command.split(' ')[0]}`);
  }
  async function submitDownload(actor, label) {
    return h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=admin-${label}`, { actor, chat: actor });
  }
  async function deniedDownload(actor, label, expectedText = 'Your account is awaiting approval. Please try again later.') {
    const before = await operations();
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, `/video http://127.0.0.1:${sourcePort}/media?case=admin-${label}`, messageId++);
    await waitFor(() => botApi.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === actor && call.data.text === expectedText), `admin denied download ${label}`);
    assert.equal(await operations(), before, `${label} must not create a runtime operation`);
  }
  async function completeDownload(actor, label) {
    const submitted = await submitDownload(actor, label);
    await h.succeeded(submitted.operationId);
    assert.equal(await fixtureSql(`SELECT actor_id FROM runtime_operations WHERE id='${submitted.operationId}'`), String(actor));
    assert(botApi.calls.slice(submitted.callStart).some(call => ['sendvideo', 'senddocument'].includes(call.method) && Number(call.chat_id) === actor),
      `${label} must deliver the actual download to the actor`);
  }
  try {
    const ownerToken = await login(42);
    const missingAuth = await request('/api/v1/bot-settings', { expected: 401, unauthenticated: true });
    assert(missingAuth.data);
    const initialSettings = await request('/api/v1/bot-settings', { token: ownerToken, expected: 200 });
    assert.equal(initialSettings.data.browser_cookies_min_role, 'owner', 'fixture starts from the current legacy browser-cookie role policy');
    await request('/api/v1/bot-settings', { token: ownerToken, method: 'PATCH', body: { bot_access_mode: 'approved_only' }, expected: 200 });

    const signedToken = await login(registeredActor);
    assert.equal((await request('/api/v1/users/me', { token: signedToken, expected: 200 })).data.id, registeredActor);
    assert.equal((await fixtureSql(`SELECT role FROM users WHERE id=${registeredActor}`)), 'restricted', 'approved-only signed registration must start restricted');
    const malformed = await request(`/api/v1/users/${registeredActor}`, { token: ownerToken, method: 'PATCH', body: '{', expected: 422 });
    assert(malformed.data);
    const registration = await send(telegramActor, '/start');
    assert.match(registration.data.text, /approval|approved|pending|access/i);
    assert.equal(await fixtureSql(`SELECT role FROM users WHERE id=${telegramActor}`), 'restricted', 'trusted private Telegram registration must start restricted');
    const nonOwner = await login(telegramActor);
    await request('/api/v1/bot-settings', { token: nonOwner, method: 'PATCH', body: { bot_access_mode: 'open' }, expected: 403 });

    await request(`/api/v1/users/${registeredActor}`, { token: ownerToken, method: 'PATCH', body: { role: 'user' }, expected: 200 });
    await completeDownload(registeredActor, 'approved');
    await request(`/api/v1/users/${registeredActor}`, { token: ownerToken, method: 'PATCH', body: { role: 'admin' }, expected: 200 });
    await request('/api/v1/features', { token: signedToken, expected: 200 });
    const available = await request('/api/v1/bot-settings/available-features', { token: signedToken, expected: 200 });
    assert(available.data.some(feature => feature.key === 'dl:inline' && feature.label && feature.description));
    await request('/api/v1/bot-settings/tag-map', { token: signedToken, expected: 200 });
    await request('/api/v1/bot-settings/tag-map', { token: signedToken, method: 'PUT', body: [], expected: 403 });
    apiKeys = await apiKeyScenarios({ request, fixtureSql, ownerToken, adminToken: signedToken, ordinaryToken: nonOwner });
    await request(`/api/v1/users/${registeredActor}`, { token: ownerToken, method: 'PATCH', body: { role: 'restricted' }, expected: 200 });
    await deniedDownload(registeredActor, 'role-revoked');
    await request('/api/v1/features', { token: signedToken, expected: 403 });
    await apiKeys.assertDenied(signedToken);
    await request('/api/v1/users/me', { token: signedToken, expected: 200 });

    const catalog = await request('/api/v1/features', { token: ownerToken, expected: 200 });
    assert(Array.isArray(catalog.data));
    const permissionFeature = catalog.data.find(feature => feature.plugin === 'dl' && feature.feature === 'inline');
    assert(permissionFeature, 'feature catalog must advertise dl/inline before the fixture grants it');
    await request('/api/v1/bot-settings/tag-map', { expected: 401 });
    await request('/api/v1/bot-settings/tag-map', { token: signedToken, expected: 403 });
    const tagEntries = [
      { tag: 'alpha', features: ['dl:inline'] },
      { tag: 'Подписчик', features: ['insight:tldr', 'stats:stats'] },
      { tag: 'Я'.repeat(129), features: [] },
    ];
    const replaced = await request('/api/v1/bot-settings/tag-map', { token: ownerToken, method: 'PUT', body: [...tagEntries].reverse(), expected: 200 });
    assert.deepEqual(replaced.data, tagEntries, 'mapping response must sort Unicode tag labels');
    for (const body of [
      '{',
      [{ tag: 'unknown', features: ['dl:missing'] }],
      [{ tag: 'duplicate', features: [] }, { tag: 'duplicate', features: ['dl:inline'] }],
      [{ tag: ' ', features: ['dl:inline'] }],
      [{ tag: 'invalid', features: 'dl:inline' }],
    ]) {
      await request('/api/v1/bot-settings/tag-map', { token: ownerToken, method: 'PUT', body, expected: 422 });
      assert.deepEqual((await request('/api/v1/bot-settings/tag-map', { token: ownerToken, expected: 200 })).data, tagEntries,
        'rejected replacement must leave stored map unchanged');
    }
    const storedMap = await fixtureSql("SELECT value FROM bot_settings WHERE key='tag_map'");
    await fixtureSql("UPDATE bot_settings SET value='{' WHERE key='tag_map'");
    await request('/api/v1/bot-settings/tag-map', { token: ownerToken, expected: 503 });
    assert.equal(await fixtureSql("SELECT value FROM bot_settings WHERE key='tag_map'"), '{', 'read must not repair corrupt data implicitly');
    await fixtureSql(`UPDATE bot_settings SET value=${literal(storedMap)} WHERE key='tag_map'`);
    const ownerBan = await fixtureSql('SELECT ban_until FROM users WHERE id=42');
    try {
      await fixtureSql("UPDATE users SET ban_until=(unixepoch('now')+600)*1000000 WHERE id=42");
      await request('/api/v1/bot-settings/tag-map', { token: ownerToken, expected: 403 });
      await apiKeys.assertDenied(ownerToken);
      await request('/api/v1/bot-settings/tag-map', { token: ownerToken, method: 'PUT', body: [], expected: 403 });
      assert.equal(await fixtureSql("SELECT value FROM bot_settings WHERE key='tag_map'"), storedMap, 'configured-owner ban must prevent replacement');
    } finally {
      await fixtureSql(`UPDATE users SET ban_until=${ownerBan || 'NULL'} WHERE id=42`);
    }
    await request(`/api/v1/users/${registeredActor}/permissions`, {
      token: ownerToken, method: 'POST', body: { plugin: permissionFeature.plugin, feature: permissionFeature.feature }, expected: 200,
    });
    const grantedAccess = await request('/api/v1/feature-access/me', { token: signedToken, expected: 200 });
    const inlineAccess = grantedAccess.data.find(feature => feature.plugin === permissionFeature.plugin && feature.feature === permissionFeature.feature);
    assert.equal(inlineAccess?.effective, true, 'dl/inline grant must take effect immediately for restricted account feature access');
    await deniedDownload(registeredActor, 'grant-does-not-bypass-role-gated-video');

    await request(`/api/v1/users/${registeredActor}`, {
      token: ownerToken, method: 'PATCH', body: { ban_until: new Date(Date.now() + 600_000).toISOString() }, expected: 200,
    });
    await deniedDownload(registeredActor, 'timed-ban-overrides-grant', 'This operation is not allowed.');

    await request(`/api/v1/users/${telegramActor}`, { token: ownerToken, method: 'PATCH', body: { role: 'user' }, expected: 200 });
    const previousRevision = await fixtureSql(`SELECT updated_at FROM users WHERE id=${telegramActor}`);
    await request(`/api/v1/users/${telegramActor}`, { token: ownerToken, method: 'PATCH', body: { language: 'ru' }, expected: 200 });
    const storedRevision = await fixtureSql(`SELECT updated_at FROM users WHERE id=${telegramActor}`);
    assert.notEqual(storedRevision, previousRevision, 'admin language update must persist a new account revision');
    const localized = await send(telegramActor, '/help');
    assert.match(localized.data.text, /\p{Script=Cyrillic}/u, 'admin-set Russian language must drive the subsequent real bot reply');
    assert.equal(await fixtureSql(`SELECT language FROM users WHERE id=${telegramActor}`), 'ru');
    await request(`/api/v1/users/${registeredActor}`, {
      token: ownerToken, method: 'PATCH', body: { ban_until: new Date(Date.now() + 600_000).toISOString() }, expected: 200,
    });
    await request(`/api/v1/users/${telegramActor}`, {
      token: ownerToken, method: 'PATCH', body: { ban_until: new Date(Date.now() + 600_000).toISOString() }, expected: 200,
    });
    const registeredBan = await fixtureSql(`SELECT ban_until FROM users WHERE id=${registeredActor}`);
    const telegramBan = await fixtureSql(`SELECT ban_until FROM users WHERE id=${telegramActor}`);
    assert.notEqual(registeredBan, '');
    assert.notEqual(telegramBan, '');

    await apiKeys.assertDenied(signedToken);
    await restartApp();
    await apiKeys.assertPersisted();
    assert.equal(await fixtureSql("SELECT value FROM bot_settings WHERE key='bot_access_mode'"), 'approved_only', 'policy survives application restart');
    assert.deepEqual((await request('/api/v1/bot-settings/tag-map', { token: ownerToken, expected: 200 })).data, tagEntries, 'tag map survives restart');
    assert.deepEqual((await request('/api/v1/bot-settings/tag-map', { token: ownerToken, method: 'PUT', body: [], expected: 200 })).data, [], 'empty replacement clears mapping');
    assert.equal(await fixtureSql(`SELECT role FROM users WHERE id=${registeredActor}`), 'restricted');
    assert.equal(await fixtureSql(`SELECT count(*) FROM user_permissions WHERE user_id=${registeredActor} AND plugin='${permissionFeature.plugin}' AND feature='${permissionFeature.feature}'`), '1', 'explicit grant survives application restart');
    assert.equal(await fixtureSql(`SELECT role||'|'||language FROM users WHERE id=${telegramActor}`), 'user|ru');
    assert.equal(await fixtureSql(`SELECT ban_until FROM users WHERE id=${registeredActor}`), registeredBan, 'registered actor timed ban survives restart unchanged');
    assert.equal(await fixtureSql(`SELECT ban_until FROM users WHERE id=${telegramActor}`), telegramBan, 'Telegram actor timed ban survives restart unchanged');
    await deniedDownload(registeredActor, 'restart-ban-persists', 'This operation is not allowed.');
    const bannedStart = await send(telegramActor, '/start');
    assert.doesNotMatch(bannedStart.data.text, /\/video|\/download|\/audio/i, 'persisted ban must keep Telegram account blocked after restart');
    assert.match(bannedStart.data.text, /\p{Script=Cyrillic}/u, 'persisted Russian language applies to the Telegram ban response');
    await request(`/api/v1/users/${registeredActor}`, { token: ownerToken, method: 'PATCH', body: { ban_until: null }, expected: 200 });
    const clearedBanAccess = await request('/api/v1/feature-access/me', { token: signedToken, expected: 200 });
    assert.equal(clearedBanAccess.data.find(feature => feature.plugin === permissionFeature.plugin && feature.feature === permissionFeature.feature)?.effective, true,
      'explicit null ban clear restores the existing feature grant without changing role');
    await deniedDownload(registeredActor, 'explicit-ban-clear-still-role-restricted');
    await request(`/api/v1/users/${registeredActor}/permissions/${permissionFeature.plugin}/${permissionFeature.feature}`, {
      token: ownerToken, method: 'DELETE', expected: 200,
    });
    const revokedAccess = await request('/api/v1/feature-access/me', { token: signedToken, expected: 200 });
    assert.equal(revokedAccess.data.find(feature => feature.plugin === permissionFeature.plugin && feature.feature === permissionFeature.feature)?.effective, false,
      'revoking dl/inline must remove effective grant access immediately');
    await deniedDownload(registeredActor, 'grant-revoked');
  } finally {
    botApi.setExpectedUpload(null);
    await apiKeys?.cleanup();
    if (savedTagMap) await fixtureSql(`UPDATE bot_settings SET value=${literal(savedTagMap)} WHERE key='tag_map'`);
    else await fixtureSql("DELETE FROM bot_settings WHERE key='tag_map'");
    if (savedPolicy) await fixtureSql(`UPDATE bot_settings SET value=${literal(savedPolicy)},updated_at=unixepoch('now')*1000000 WHERE key='bot_access_mode'`);
    else await fixtureSql("DELETE FROM bot_settings WHERE key='bot_access_mode'");
  }
  return 'authenticated administration, tag-map replacement and catalog, API-key generation/nonexposure/revocation/restart, approved-only registration, immediate role/grant/ban enforcement, persisted policy and localized Telegram behavior verified';
}
