import assert from 'node:assert/strict';
import { createHmac, randomInt } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { commandHarness } from './command-harness.mjs';

export async function runMembershipScenarios(context) {
  const { botApi, fixtureSql, waitFor, restartApp, appHttpPort, sourcePort, telegramToken } = context;
  const h = commandHarness(context, 2_100_000);
  const user = 7_200_000_000_000 + randomInt(1_000_000_000);
  const group = -user;
  const otherGroup = group - 1;
  const base = `http://127.0.0.1:${appHttpPort}/api/v1`;
  let messageId = 2_200_000;
  let date = Math.floor(Date.now() / 1000);
  async function request(path, { token, method = 'GET', body, status = 200 } = {}) {
    const response = await fetch(base + path, {
      method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(body === undefined ? {} : { 'content-type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    assert.equal(response.status, status, `${method} ${path}`);
    const text = await response.text();
    return text ? JSON.parse(text) : null;
  }
  async function login(id) {
    const authDate = String(Math.floor(Date.now() / 1000));
    const profile = JSON.stringify({ id, first_name: 'Membership fixture' });
    const key = createHmac('sha256', 'WebAppData').update(telegramToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${authDate}\nuser=${profile}`).digest('hex');
    return (await request('/auth/token', { method: 'POST', body: { init_data: new URLSearchParams({ auth_date: authDate, user: profile, hash }).toString() } })).access_token;
  }
  async function observe(chat, subject, status, options = {}) {
    const updateId = botApi.enqueueMembership(chat, subject, status, { date: ++date, ...options });
    await waitFor(() => botApi.polls.some(poll => poll.offset > updateId), `membership offset ${updateId}`);
    return updateId;
  }
  async function menu(allowed, label) {
    const scope = { type: 'chat_member', chat_id: group, user_id: user };
    await waitFor(() => {
      const commands = botApi.commandMenus.get({ scope });
      return botApi.commandMenus.has({ scope }) && (allowed ? commands.some(command => command.command === 'video') : commands.length === 0);
    }, `membership menu ${label}`);
  }
  async function denied(label) {
    const operations = await fixtureSql('SELECT count(*) FROM runtime_operations');
    const start = botApi.calls.length;
    botApi.enqueue(user, group, `/video http://127.0.0.1:${sourcePort}/media?case=${label}`, messageId++);
    botApi.enqueue(42, 42, '/help', messageId++);
    await waitFor(() => botApi.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 42), `membership denial control ${label}`);
    await sleep(800);
    assert(!botApi.calls.slice(start).some(call => ['sendmessage', 'sendvideo', 'senddocument'].includes(call.method) && Number(call.data?.chat_id ?? call.chat_id) === group), `${label} must remain silent`);
    assert.equal(await fixtureSql('SELECT count(*) FROM runtime_operations'), operations, `${label} must not create a job`);
  }
  async function allowed(label) {
    const submitted = await h.submit(`/video http://127.0.0.1:${sourcePort}/media?case=${label}`, { actor: user, chat: group });
    await h.succeeded(submitted.operationId);
    assert.equal(await fixtureSql(`SELECT role FROM users WHERE id=${user}`), 'restricted', 'membership must never escalate global role');
  }
  const owner = await login(42);
  let ordinary;
  const feature = async () => {
    const response = await request('/feature-access/me', { token: ordinary });
    return response.find(row => row.plugin === 'dl' && row.feature === 'inline')?.effective;
  };
  const permission = (method, status = 200) => request(`/users/${user}/permissions${method === 'DELETE' ? '/dl/inline' : ''}`, { token: owner, method, ...(method === 'POST' ? { body: { plugin: 'dl', feature: 'inline' } } : {}), status });
  const tagMap = entries => request('/bot-settings/tag-map', { token: owner, method: 'PUT', body: entries });
  try {
    await observe(group, 123456, 'member', { bot: true });
    const discovered = await request(`/groups/${group}`, { token: owner });
    assert.equal(discovered.enabled, false, 'bot addition must discover a disabled group');
    assert.equal(discovered.auto_grant_role, 'user');
    await request(`/groups/${group}`, { token: owner, method: 'PATCH', body: { enabled: true, auto_grant_role: 'user', allow_pm: true } });
    await observe(group, user, 'member', { tag: 'alpha' });
    ordinary = await login(user);
    await request(`/users/${user}`, { token: owner, method: 'PATCH', body: { role: 'restricted' } });
    await menu(true, 'present');
    await allowed('membership-local-grant');
    assert(botApi.polls.some(poll => ['message', 'callback_query', 'my_chat_member', 'chat_member'].every(kind => poll.allowed_updates.includes(kind))), 'poller must request membership updates explicitly');

    await tagMap([{ tag: 'alpha', features: ['dl:inline'] }]);
    assert.equal(await feature(), true, 'stored membership tag must apply when the map is edited');
    await permission('DELETE');
    assert.equal(await feature(), false, 'manual revoke must suppress tag grant');
    await observe(group, user, 'member', { tag: 'alpha' });
    assert.equal(await feature(), false, 'tag refresh must not undo manual revoke');
    await restartApp();
    assert.equal(await feature(), false, 'tag suppression must survive restart');
    await permission('POST');
    assert.equal(await feature(), true, 'explicit restore clears suppression');
    await observe(group, user, 'member');
    assert.equal(await feature(), true, 'tag removal must preserve manual permission');
    await permission('DELETE');
    assert.equal(await feature(), false);
    await permission('POST');
    await tagMap([{ tag: 'alpha', features: ['dl:inline'] }, { tag: 'beta', features: ['stats:stats'] }]);
    await observe(group, user, 'member', { tag: 'alpha' });
    // Remove only manual provenance to check independent automatic sources.
    await fixtureSql(`DELETE FROM user_permissions WHERE user_id=${user} AND plugin='dl' AND feature='inline'`);
    assert.equal(await feature(), true);
    await observe(otherGroup, 123456, 'member', { bot: true });
    await request(`/groups/${otherGroup}`, { token: owner, method: 'PATCH', body: { enabled: true } });
    await observe(otherGroup, user, 'member', { tag: 'alpha' });
    await observe(group, user, 'member', { tag: 'beta' });
    assert.equal(await feature(), true, 'second eligible group retains matching tag grant');
    await observe(otherGroup, user, 'left');
    assert.equal(await feature(), false, 'last matching tag source leaving must revoke automatic permission');
    await observe(group, user, 'member', { tag: 'alpha' });
    assert.equal(await feature(), true);
    await observe(group, user, 'left');
    assert.equal(await feature(), false, 'member departure revokes derived permission');
    await menu(false, 'member-left');
    await denied('membership-member-left');
    await observe(group, user, 'member', { tag: 'alpha', date: date - 100 });
    assert.equal(await feature(), false, 'newer update id with stale event date must not undo departure');
    await denied('membership-stale-join');
    await observe(group, user, 'restricted', { present: false });
    await denied('membership-restricted-absent');
    await observe(group, user, 'restricted', { present: true, tag: 'alpha' });
    await menu(true, 'restricted-present');
    assert.equal(await feature(), true);
    await allowed('membership-restricted-present');

    await observe(group, 123456, 'kicked', { bot: true });
    assert.equal((await request(`/groups/${group}`, { token: owner })).enabled, true, 'bot removal must not change administrative approval');
    assert.equal(await feature(), false, 'bot absence revokes derived permission');
    await menu(false, 'bot-left');
    await denied('membership-bot-left');
    await observe(group, 123456, 'member', { bot: true });
    await menu(false, 'bot-rejoined-stale-membership');
    assert.equal(await feature(), false, 'rejoin must not revive tags from the old bot membership generation');
    await observe(group, user, 'member', { tag: 'alpha' });
    await menu(true, 'bot-rejoined-fresh-membership');
    assert.equal(await feature(), true);
    await restartApp();
    await menu(true, 'restart');
    assert.equal(await feature(), true);
    await request(`/groups/${group}`, { token: owner, method: 'DELETE', status: 204 });
    await menu(false, 'group-deleted');
    assert.equal(await feature(), false);
    await observe(group, 123456, 'member', { bot: true });
    await request(`/groups/${group}`, { token: owner, status: 404 });
    await denied('membership-deleted-group-stays-deleted');
    await request('/groups', { token: owner, method: 'POST', body: { id: group, title: 'Explicitly restored' }, status: 201 });
    assert.equal((await request(`/groups/${group}`, { token: owner })).enabled, false, 'explicit restore still requires approval');
  } finally {
    for (const id of [group, otherGroup]) {
      await request(`/groups/${id}`, { token: owner, method: 'DELETE', status: (await fixtureSql(`SELECT count(*) FROM groups WHERE id=${id}`)) === '0' ? 404 : 204 });
    }
    botApi.setExpectedUpload(null);
  }
  return 'raw membership polling, transactional acknowledgments, disabled discovery, local role isolation, tag provenance and suppression, stale observation fences, bot generations, departure execution/menu revocation and restart verified';
}
