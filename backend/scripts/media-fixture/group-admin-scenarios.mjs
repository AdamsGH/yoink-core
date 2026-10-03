import assert from 'node:assert/strict';
import { createHmac, randomInt } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runGroupAdminScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, appHttpPort, sourcePort, telegramToken } = context;
  const h = commandHarness(context, 1_900_000);
  const user = 7_100_000_000_000 + randomInt(1_000_000_000);
  const group = -user;
  const otherGroup = group - 1;
  const base = `http://127.0.0.1:${appHttpPort}`;
  let messageId = 2_000_000;
  async function request(path, { token, method = 'GET', body, status = 200 } = {}) {
    const response = await fetch(base + '/api/v1' + path, {
      method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    assert.equal(response.status, status, `${method} ${path}`);
    if (status !== 204) assert.equal(response.headers.get('cache-control'), 'no-store');
    const text = await response.text();
    return text ? JSON.parse(text) : null;
  }
  async function login(id) {
    const authDate = String(Math.floor(Date.now() / 1000));
    const profile = JSON.stringify({ id, first_name: 'Group fixture' });
    const key = createHmac('sha256', 'WebAppData').update(telegramToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${authDate}\nuser=${profile}`).digest('hex');
    const result = await request('/auth/token', { method: 'POST', body: { init_data: new URLSearchParams({ auth_date: authDate, user: profile, hash }).toString() } });
    return result.access_token;
  }
  async function menu(allowed, label) {
    const scope = { type: 'chat_member', chat_id: group, user_id: user };
    await waitFor(() => {
      const commands = botApi.commandMenus.get({ scope });
      return botApi.commandMenus.has({ scope }) && (allowed ? commands.some(command => command.command === 'video') : commands.length === 0);
    }, `group menu ${label}`);
    const commands = botApi.commandMenus.get({ scope }).map(command => command.command);
    for (const name of ['status', 'list', 'settings', 'format', 'quality', 'proxy', 'cookie', 'lang']) assert(!commands.includes(name), `${label}: private command ${name} must not appear in group menus`);
    for (const language_code of ['en', 'ru']) assert(!botApi.commandMenus.has({ scope, language_code }), `${label}: stale language override`);
  }
  async function denied(thread, label) {
    const operations = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const start = botApi.calls.length;
    botApi.enqueue(user, group, `/video http://127.0.0.1:${sourcePort}/media?case=${label}`, messageId++, 'en', { thread });
    // The private control proves the denial is not just a stalled poller.
    botApi.enqueue(42, 42, '/help', messageId++);
    await waitFor(() => botApi.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), `group admin denial control ${label}`);
    await sleep(800);
    assert(!botApi.calls.slice(start).some(call => ['sendmessage', 'sendvideo', 'senddocument'].includes(call.method) && Number(call.data?.chat_id ?? call.chat_id) === group), `${label} must remain silent`);
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), operations, `${label} must not create a job`);
  }
  async function allowed(thread, label) {
    const submitted = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=${label}`, { actor: user, chat: group, thread });
    await h.succeeded(submitted.operationId);
    const media = botApi.calls.slice(submitted.callStart).find(call => ['sendvideo', 'senddocument'].includes(call.method));
    assert(media, `${label} must deliver media`);
    assert.equal(Number(media.chat_id), group);
    assert.equal(media.message_thread_id, thread);
    assert.equal(await fixtureSql(`SELECT role FROM users WHERE id=${user}`), 'restricted', 'group grants must not mutate global role');
  }
  const owner = await login(42);
  const ordinary = await login(user);
  await request(`/users/${user}`, { token: owner, method: 'PATCH', body: { role: 'restricted' } });
  const create = id => request('/groups', { token: owner, method: 'POST', body: { id, title: 'Native group fixture', auto_grant_role: 'restricted', allow_pm: false, nsfw_allowed: true }, status: 201 });
  try {
    await request('/groups', { status: 401 });
    await request('/groups', { token: ordinary, status: 403 });
    const initial = await create(group);
    assert.equal(initial.id, group);
    assert.equal(initial.title, 'Native group fixture');
    assert.equal(initial.auto_grant_role, 'restricted');
    assert.equal(initial.allow_pm, false);
    assert.equal(initial.nsfw_allowed, true);
    assert.deepEqual(initial.thread_policies, []);
    assert.equal(initial.enabled, false, 'new groups must require administrator approval');
    await request(`/groups/${group}`, { token: owner, method: 'PATCH', body: { enabled: true } });
    await create(otherGroup);
    await request('/groups', { token: owner, method: 'POST', body: { id: group }, status: 409 });
    const groups = await request('/groups', { token: owner });
    assert(groups.every(row => row.id < 0), 'private-chat groups must not leak into the group list');
    assert(groups.some(row => row.id === group));
    await denied(18, 'group-no-local-grant');
    await request(`/groups/${group}/members/${user}`, { token: owner, method: 'PUT', body: { role_override: 'user', allow_pm_override: true } });
    assert.equal((await request(`/groups/${group}/members`, { token: owner })).find(row => row.user_id === user)?.role_override, 'user');
    assert.deepEqual(await request(`/groups/${otherGroup}/members`, { token: owner }), [], 'overrides must be local to one group');
    await menu(true, 'member-grant');
    await allowed(18, 'group-member-grant');

    const topic = await request(`/groups/${group}/threads`, { token: owner, method: 'POST', body: { thread_id: 18, name: 'Allowed topic', enabled: true }, status: 201 });
    const main = await request(`/groups/${group}/threads`, { token: owner, method: 'POST', body: { thread_id: null, name: 'Main chat', enabled: false }, status: 201 });
    await denied(undefined, 'group-main-chat-disabled');
    await denied(19, 'group-unlisted-topic');
    await allowed(18, 'group-listed-topic');
    const upsert = await request(`/groups/${group}/threads`, { token: owner, method: 'POST', body: { thread_id: 18, enabled: false }, status: 201 });
    assert.equal(upsert.id, topic.id);
    assert.equal(upsert.name, 'Allowed topic');
    assert.equal((await request(`/groups/${group}/threads`, { token: owner })).length, 2);
    await denied(18, 'group-disabled-topic');
    await request(`/groups/${otherGroup}/threads/${topic.id}`, { token: owner, method: 'DELETE', status: 404 });
    await request(`/groups/${group}/threads/${topic.id}`, { token: owner, method: 'DELETE', status: 204 });
    await allowed(19, 'group-no-positive-whitelist');
    await request(`/groups/${group}`, { token: owner, method: 'PATCH', body: { enabled: false } });
    await menu(false, 'disabled-via-api');
    await denied(19, 'group-disabled-via-api');
    await request(`/groups/${group}`, { token: owner, method: 'PATCH', body: { enabled: true } });
    await request(`/groups/${group}/members/${user}`, { token: owner, method: 'DELETE', status: 204 });
    await menu(false, 'member-revoked');
    await denied(19, 'group-member-revoked');
    await request(`/groups/${group}`, { token: owner, method: 'PATCH', body: { auto_grant_role: 'user' } });
    await menu(true, 'default-grant');
    await allowed(19, 'group-default-grant');
    await request(`/users/${user}`, { token: owner, method: 'PATCH', body: { ban_until: new Date(Date.now() + 600_000).toISOString() } });
    await menu(false, 'global-ban');
    await denied(19, 'global-ban-overrides-group-grant');
    await request(`/users/${user}`, { token: owner, method: 'PATCH', body: { ban_until: null } });
    await restartApp();
    const persisted = await request(`/groups/${group}`, { token: owner });
    assert.equal(persisted.auto_grant_role, 'user');
    assert(persisted.thread_policies.some(row => row.id === main.id && row.thread_id === null && row.enabled === false));
    await menu(true, 'restart');
    await allowed(19, 'group-policy-persists-after-restart');
    await request(`/groups/${group}`, { token: owner, method: 'DELETE', status: 204 });
    await request(`/groups/${group}`, { token: owner, status: 404 });
    await menu(false, 'deleted-group');
    await denied(19, 'group-deletion-revokes-admission');
    assert.equal(await fixtureSql(`SELECT count(*) FROM thread_policies WHERE group_id=${group}`), '0');
    assert.equal(await fixtureSql(`SELECT count(*) FROM user_group_policies WHERE group_id=${group}`), '0');
  } finally {
    for (const id of [group, otherGroup]) await request(`/groups/${id}`, { token: owner, method: 'DELETE', status: (await fixtureSql(`SELECT count(*) FROM groups WHERE id=${id}`)) === '0' ? 404 : 204 });
    botApi.setExpectedUpload(null);
  }
  return 'native group/topic/member HTTP administration, immediate Telegram admission and revocation, local role isolation, global bans and persisted policies verified';
}
