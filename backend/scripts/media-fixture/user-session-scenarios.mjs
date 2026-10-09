import assert from 'node:assert/strict';
import http from 'node:http';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';

export async function startUserSessionFixture(installToken, registerSecret = () => {}) {
  let token = randomBytes(32).toString('hex');
  let mode = 'ok';
  const calls = [];
  let callCount = 0;
  let failure;
  const handlers = new Set();
  const gates = new Set();
  let nextMemberGate;
  const member = id => ({ user: { id, username: `session_${id}`, firstName: 'Session', lastName: String(id) }, status: 'member', joined_date: 1700000000 });
  const topic = id => ({ message_thread_id: id, name: `Disposable topic ${id}` });
  function result(method, params, responseMode) {
    switch (method) {
      case 'getForumTopics': {
        if (responseMode === 'topics-over-limit') return { topics: Array.from({ length: 101 }, (_, i) => topic(i + 1)) };
        if (['paged', 'cursor-repeat'].includes(responseMode)) {
          const offset = Number(params.offset_forum_topic_id ?? 0);
          return offset && responseMode === 'paged'
            ? { topics: [topic(101)], total_count: 101 }
            : { topics: Array.from({ length: 100 }, (_, i) => topic(i + 1 + (offset ? 100 : 0))), total_count: 101, next_offset_date: 1, next_offset_message_id: 100, next_offset_forum_topic_id: 100 };
        }
        return { topics: [topic(77)], total_count: 1 };
      }
      case 'getChatMembers': {
        if (params.filter !== 'members' || params.limit !== '200' || !/^\d+$/.test(params.offset ?? '')) return undefined;
        const offset = Number(params.offset);
        const count = responseMode === 'members-over-limit' ? 201 : Math.max(0, Math.min(200, 201 - offset));
        return Array.from({ length: count }, (_, i) => member(offset + i < 2 ? 42 + offset + i : 880000 + offset + i));
      }
      case 'getForumTopic': return topic(Number(params.message_thread_id));
      case 'getForumTopicLink': return { link: `https://t.me/c/8000000000011/${params.message_thread_id}` };
      case 'searchChatMessages':
      case 'getChatHistory': return { messages: [{ message_id: 501, text: 'Disposable message' }], total_count: 1 };
      case 'getMessageViewers': return [{ user_id: 880000, view_date: 1700000000 }];
      case 'getMessageLink': return { link: `https://t.me/c/8000000000011/${params.message_id}` };
      case 'getMessageReadDate': return { read_date: 1700000000 };
      case 'getMessageThread': return { message_thread_id: 77, messages: [{ message_id: Number(params.message_id) }] };
      case 'getChatMessageByDate': return { message_id: 501, date: Number(params.date) };
      default: return undefined;
    }
  }
  const server = http.createServer((req, res) => {
    const running = handle(req, res).catch(() => {
      if (!req.aborted) failure ??= new Error('tdlight fixture request failed');
      res.destroy();
    }).finally(() => handlers.delete(running));
    handlers.add(running);
  });
  async function handle(req, res) {
    req.setTimeout(3000, () => req.destroy());
    let body = '';
    for await (const chunk of req) {
      body += chunk;
      if (body.length > 8192) { res.writeHead(413).end(); return; }
    }
    const method = req.url?.split('/').at(-1);
    if (req.method !== 'POST' || req.url !== `/user${token}/${method}`) { res.writeHead(401).end(); return; }
    calls.push({ method, params: Object.fromEntries(new URLSearchParams(body)) });
    callCount = Math.min(Number.MAX_SAFE_INTEGER, callCount + 1);
    if (calls.length > 64) calls.shift();
    if (mode === 'error') { res.writeHead(502).end(); return; }
    const responseMode = mode;
    const value = result(method, calls.at(-1).params, responseMode);
    if (value === undefined) { res.writeHead(400).end(); return; }
    if (method === 'getChatMembers' && nextMemberGate) {
      const gate = nextMemberGate;
      nextMemberGate = undefined;
      gate.entered = true;
      await gate.promise;
    }
    res.setHeader('content-type', 'application/json');
    res.end(JSON.stringify(responseMode === 'malformed' ? { ok: true, result: null } : { ok: true, result: value }));
  }
  async function close() {
    for (const gate of gates) gate.release();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    await Promise.all([...handlers]);
    if (failure) throw failure;
  }
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  try {
    registerSecret(token);
    await installToken(token);
  } catch (error) {
    await close();
    throw error;
  }
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    calls,
    get callCount() { return callCount; },
    get pendingRequests() { return handlers.size; },
    assertHealthy() { if (failure) throw failure; },
    setMode(value) { assert(['ok', 'error', 'malformed', 'paged', 'cursor-repeat', 'topics-over-limit', 'members-over-limit'].includes(value)); mode = value; },
    holdNextMembers() {
      assert(!nextMemberGate, 'only one pending member gate');
      let resolve;
      const gate = { entered: false, promise: new Promise(r => { resolve = r; }), release() { resolve(); gates.delete(gate); if (nextMemberGate === gate) nextMemberGate = undefined; } };
      gates.add(gate);
      nextMemberGate = gate;
      return gate;
    },
    async setAvailable(value) { await installToken(value ? token : ''); },
    async rotate() { token = randomBytes(32).toString('hex'); registerSecret(token); await installToken(token); },
    close,
  };
}

export async function runUserSessionScenarios({ request, fixture, owner, ordinary, fixtureSql, waitFor, context }) {
  assert.equal(typeof waitFor, 'function', 'coordinator must supply waitFor');
  assert(context?.botApi, 'coordinator must supply context.botApi');
  assert.deepEqual((await request('/api/v1/threads/status', owner)).data, { available: true });
  assert.equal((await request('/api/v1/threads/status', ordinary)).status, 403);
  const chat = -8000000000011;
  const route = `/api/v1/forum/topics/${chat}`;
  const before = fixture.callCount;
  assert.equal((await request(route, ordinary)).status, 403);
  assert.equal(fixture.callCount, before, 'denied authority must not contact tdlight');
  const scanned = await request(`/api/v1/threads/scan/${chat}`, owner, { method: 'POST' });
  assert.equal(scanned.status, 200);
  assert.equal(scanned.data.topics[0].message_thread_id, 77);
  assert.equal(scanned.data.upserted, 1);
  assert.equal((await request(route, owner)).status, 200);
  await fixture.rotate();
  assert.equal((await request(route, owner)).status, 200, 'client must reread rotated protected token');
  const routes = [
    [`${route}/77`, 'getForumTopic', { chat_id: String(chat), message_thread_id: '77' }],
    [`${route}/77/link`, 'getForumTopicLink', { message_thread_id: '77' }],
    [`/api/v1/forum/search/${chat}?query=disposable&thread_id=77&limit=2&filter=`, 'searchChatMessages', { query: 'disposable', message_thread_id: '77', limit: '2' }],
    [`/api/v1/forum/history/${chat}?limit=2&only_local=true`, 'getChatHistory', { limit: '2', only_local: 'true' }],
    ...['viewers', 'link', 'read-date', 'thread'].map((suffix, i) => [`/api/v1/messages/${chat}/501/${suffix}`, ['getMessageViewers', 'getMessageLink', 'getMessageReadDate', 'getMessageThread'][i], { message_id: '501' }]),
    [`/api/v1/messages/${chat}/by-date?date=1700000000`, 'getChatMessageByDate', { date: '1700000000' }],
  ];
  for (const [path, method, params] of routes) {
    const deniedBefore = fixture.callCount;
    assert.equal((await request(path, ordinary)).status, 403);
    assert.equal(fixture.callCount, deniedBefore);
    const response = await request(path, owner);
    assert.equal(response.status, 200, path);
    assert.deepEqual(response.data, method === 'getMessageViewers' ? [{ user_id: 880000, view_date: 1700000000 }]
      : method.endsWith('Link') ? { link: `https://t.me/c/8000000000011/${method === 'getForumTopicLink' ? 77 : 501}` }
      : method === 'getForumTopic' ? { message_thread_id: 77, name: 'Disposable topic 77' }
      : method === 'getMessageReadDate' ? { read_date: 1700000000 }
      : method === 'getMessageThread' ? { message_thread_id: 77, messages: [{ message_id: 501 }] }
      : method === 'getChatMessageByDate' ? { message_id: 501, date: 1700000000 }
      : { messages: [{ message_id: 501, text: 'Disposable message' }], total_count: 1 });
    const call = fixture.calls.findLast(call => call.method === method);
    for (const [key, value] of Object.entries(params)) assert.equal(call.params[key], value);
    if (method === 'searchChatMessages') assert(!('filter' in call.params) && !('thread_id' in call.params));
  }
  for (const query of ['limit=101', 'limit=0', 'offset=-1', 'only_local=maybe', 'unknown=1']) {
    const count = fixture.callCount;
    assert.equal((await request(`/api/v1/forum/history/${chat}?${query}`, owner)).status, 422);
    assert.equal(fixture.callCount, count, 'invalid query must not reach provider');
  }
  await fixture.setAvailable(false);
  try {
    assert.deepEqual((await request('/api/v1/threads/status', owner)).data, { available: false });
    for (const path of [route, ...routes.map(([path]) => path)]) assert.equal((await request(path, owner)).status, 503);
    assert.equal((await request(`/api/v1/threads/scan/${chat}`, owner, { method: 'POST' })).status, 503);
    assert.equal((await request(`/api/v1/stats/members/sync?chat_id=${chat}`, owner, { method: 'POST' })).status, 503);
  } finally { await fixture.setAvailable(true); }
  fixture.setMode('paged');
  try {
    const pageStart = fixture.callCount;
    const paged = await request(`/api/v1/threads/scan/${chat}`, owner, { method: 'POST' });
    assert.equal(paged.status, 200);
    assert.equal(paged.data.upserted, 101);
    assert.equal(paged.data.topics.length, 101);
    assert.equal(fixture.callCount - pageStart, 2);
    assert.equal(fixture.calls.findLast(call => call.method === 'getForumTopics').params.offset_forum_topic_id, '100');
    for (const mode of ['cursor-repeat', 'topics-over-limit', 'malformed']) {
      fixture.setMode(mode);
      assert.equal((await request(`/api/v1/threads/scan/${chat}`, owner, { method: 'POST' })).status, 502);
    }
  } finally { fixture.setMode('ok'); }
  await runMemberScenarios({ request, fixture, owner, ordinary, fixtureSql, waitFor, botApi: context.botApi, chat });
  fixture.setMode('error');
  try { assert.equal((await request(route, owner)).status, 502); }
  finally { fixture.setMode('ok'); }
  await fixtureSql("UPDATE users SET ban_until=(unixepoch('now')+3600)*1000000 WHERE id=42");
  try {
    const count = fixture.callCount;
    assert.equal((await request(route, owner)).status, 403, 'owner JWT must use fresh authority');
    assert.equal((await request(`/api/v1/stats/members/sync?chat_id=${chat}`, owner, { method: 'POST' })).status, 403);
    assert.equal(fixture.callCount, count, 'fresh banned authority must not contact provider');
  }
  finally { await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=42'); }
  fixture.assertHealthy();
  return 'configured public thread/forum/message proxies, bounded scans, unavailable token, Stats roster paging/name refresh with live revision fencing and floors, authority/upstream and protected token rotation';
}

async function runMemberScenarios({ request, fixture, owner, ordinary, fixtureSql, waitFor, botApi, chat }) {
  const route = `/api/v1/stats/members/sync?chat_id=${chat}`;
  const sync = actor => request(route, actor, { method: 'POST' });
  // Read core authority, never synthesize provider output with fixture SQL.
  const core = async () => Promise.all([
    fixtureSql('SELECT id,role,ban_until FROM users ORDER BY id'),
    fixtureSql('SELECT * FROM user_permissions ORDER BY id'),
    fixtureSql('SELECT * FROM user_group_policies ORDER BY id'),
    fixtureSql('SELECT * FROM insight_access ORDER BY user_id'),
  ]);
  const baseline = await core();
  const before = fixture.callCount;
  assert.equal((await sync(ordinary)).status, 403);
  assert.equal(fixture.callCount, before);
  assert.equal((await sync(owner)).status, 200);
  const pages = fixture.calls.filter(call => call.method === 'getChatMembers').slice(-2);
  assert.deepEqual(pages.map(call => call.params), [0, 200].map(offset => ({ chat_id: String(chat), filter: 'members', offset: String(offset), limit: '200' })));
  assert.equal(await fixtureSql(`SELECT count(*) FROM stats_group_members WHERE chat_id=${chat} AND (user_id IN (42,43) OR user_id BETWEEN 880002 AND 880200) AND in_chat=1`), '201');
  assert.equal(await fixtureSql("SELECT count(*) FROM stats_user_latest_name WHERE (user_id IN (42,43) OR user_id BETWEEN 880002 AND 880200) AND username='session_'||user_id"), '201');
  assert.equal(await fixtureSql('SELECT joined_date FROM stats_group_members WHERE user_id=42 AND chat_id=' + chat), '1700000000000000');
  const floor = Number(await fixtureSql(`SELECT event_floor FROM stats_roster_floors WHERE chat_id=${chat}`));
  assert(floor > 0);
  assert(Number(await fixtureSql('SELECT refresh_floor FROM stats_name_baselines WHERE user_id=42')) > 0);
  assert.deepEqual(await core(), baseline, 'roster provider cannot create core identities, roles or grants');
  const gate = fixture.holdNextMembers();
  const pending = sync(owner);
  let liveRevision;
  try {
    await waitFor(() => gate.entered, 'real configured roster request held');
    const update = botApi.enqueueMembership(chat, 42, 'left');
    await waitFor(async () => Number(await fixtureSql(`SELECT update_id FROM stats_ingestion_watermarks WHERE kind='member' AND chat_id=${chat} AND subject_id=42`)) === update, 'live membership committed during roster lookup');
    liveRevision = Number(await fixtureSql(`SELECT revision FROM stats_subject_revisions WHERE kind='member' AND chat_id=${chat} AND subject_id=42`));
    assert.equal(await fixtureSql('SELECT display_name FROM stats_user_latest_name WHERE user_id=42'), 'Observed member');
  } finally { gate.release(); await pending.catch(() => {}); }
  assert.equal((await pending).status, 200);
  assert.equal(await fixtureSql(`SELECT in_chat||'|'||status FROM stats_group_members WHERE chat_id=${chat} AND user_id=42`), '0|left', 'stale roster must not overwrite live membership');
  assert.equal(await fixtureSql('SELECT display_name FROM stats_user_latest_name WHERE user_id=42'), 'Observed member', 'stale roster must not overwrite live name');
  assert(Number(await fixtureSql(`SELECT revision FROM stats_refresh_fences WHERE kind='roster' AND chat_id=${chat}`)) > liveRevision);
  const previousAcknowledgement = botApi.beforePollAcknowledgement;
  let old;
  let acknowledged = false;
  botApi.beforePollAcknowledgement = async offset => {
    await previousAcknowledgement?.(offset);
    if (old !== undefined && offset > old) acknowledged = true;
  };
  try {
    old = botApi.enqueueMembership(chat, 43, 'left', { date: Math.floor(floor / 1000000) - 1 });
    await waitFor(() => acknowledged, 'older membership update acknowledged');
  } finally { botApi.beforePollAcknowledgement = previousAcknowledgement; }
  assert.equal(await fixtureSql(`SELECT in_chat||'|'||status FROM stats_group_members WHERE chat_id=${chat} AND user_id=43`), '1|member', 'roster floor rejects old live membership');
  assert.equal(await fixtureSql('SELECT username FROM stats_user_latest_name WHERE user_id=43'), 'session_43', 'name refresh floor rejects old live name');
  for (const mode of ['error', 'members-over-limit', 'malformed']) {
    const rows = await fixtureSql(`SELECT * FROM stats_group_members WHERE chat_id=${chat} ORDER BY user_id`);
    const fence = await fixtureSql(`SELECT revision FROM stats_refresh_fences WHERE kind='roster' AND chat_id=${chat}`);
    const names = await fixtureSql('SELECT * FROM stats_user_names WHERE user_id IN (42,43) OR user_id BETWEEN 880002 AND 880200 ORDER BY id');
    fixture.setMode(mode);
    try { assert.equal((await sync(owner)).status, 502); }
    finally { fixture.setMode('ok'); }
    assert.equal(await fixtureSql(`SELECT * FROM stats_group_members WHERE chat_id=${chat} ORDER BY user_id`), rows, 'failed roster cannot partially reconcile');
    assert.equal(await fixtureSql(`SELECT revision FROM stats_refresh_fences WHERE kind='roster' AND chat_id=${chat}`), fence);
    assert.equal(await fixtureSql('SELECT * FROM stats_user_names WHERE user_id IN (42,43) OR user_id BETWEEN 880002 AND 880200 ORDER BY id'), names);
  }
  assert.deepEqual(await core(), baseline, 'Stats provider/live observation cannot create core identities, roles or grants');
}
