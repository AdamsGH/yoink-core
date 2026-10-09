import assert from 'node:assert/strict';
import http from 'node:http';
import { createHmac } from 'node:crypto';
import { runUserSessionScenarios } from './user-session-scenarios.mjs';

export const desktopChat = -8000000000099;
export const desktopMessages = 60000;

export function desktopExport(count = desktopMessages) {
  assert(Number.isInteger(count) && count > 0 && count <= 100000);
  const bytes = JSON.stringify({ messages: Array.from({ length: count }, (_, i) => ({
    id: i + 1, type: 'message', date: '2025-01-01T00:00:00', from_id: 'user42', text: `native atomic import ${i}`,
  })) });
  assert(Buffer.byteLength(bytes) < 16 * 1024 * 1024);
  return bytes;
}

export function assertAtomicRows(count, total) {
  assert([0, total].includes(Number(count)), `Desktop rows must be rollback-or-complete, got ${count}/${total}`);
}

// Send multipart headers and a partial file, deliberately never sending the final boundary.
export function holdImportBody(base, token) {
  const request = http.request(`${base}/api/v1/stats/import?chat_id=${desktopChat}`, {
    method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'multipart/form-data; boundary=native-held' },
  });
  const done = new Promise(resolve => {
    request.once('response', response => {
      response.resume();
      response.once('end', () => resolve({ status: response.statusCode }));
      response.once('error', error => resolve({ error: error.code }));
    });
    request.once('error', error => resolve({ error: error.code }));
  });
  request.setTimeout(25000, () => request.destroy(new Error('held import deadline exceeded')));
  request.write('--native-held\r\nContent-Disposition: form-data; name="file"; filename="held.json"\r\nContent-Type: application/json\r\n\r\n{"messages":[');
  return { done, close: () => request.destroy() };
}

export async function runNativeProductsScenarios(context) {
  const { botApi, fixtureSql, waitFor, appHttpPort, telegramToken, enableProducts, stopProducts, reopenProducts } = context;
  const base = `http://127.0.0.1:${appHttpPort}`;
  async function request(route, token, options = {}) {
    const response = await fetch(base + route, { ...options, headers: { ...options.headers, ...(token ? { authorization: `Bearer ${token}` } : {}) }, signal: AbortSignal.timeout(15000) });
    const text = await response.text();
    return { status: response.status, data: text ? JSON.parse(text) : null };
  }
  async function login(id) {
    const user = JSON.stringify({ id, first_name: 'Native fixture' });
    const auth_date = String(Math.floor(Date.now() / 1000));
    const key = createHmac('sha256', 'WebAppData').update(telegramToken).digest();
    const hash = createHmac('sha256', key).update(`auth_date=${auth_date}\nuser=${user}`).digest('hex');
    const result = await request('/api/v1/auth/token', undefined, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ init_data: new URLSearchParams({ auth_date, user, hash }).toString() }) });
    assert.equal(result.status, 200);
    return result.data.access_token;
  }
  async function send(actor, command) {
    const start = botApi.calls.length;
    botApi.enqueue(actor, actor, command);
    return waitFor(() => botApi.calls.slice(start).find(call => call.method === 'sendmessage' && Number(call.data.chat_id) === actor), `native ${command} reply`);
  }
  const owner = await login(42);
  const ordinary = await login(43);
  assert.equal((await request('/api/v1/music/me/stats', owner)).status, 404, 'Music is disabled by default');
  assert.equal((await request('/api/v1/stats/groups', owner)).status, 404, 'Stats is disabled by default');
  await enableProducts();
  assert.equal((await request('/health/ready')).status, 200);
  for (const route of ['/api/v1/music/me/stats', '/api/v1/stats/groups']) {
    assert.equal((await request(route)).status, 401);
    assert.equal((await request(route, owner)).status, 200);
  }
  assert.match((await send(42, '/stats')).data.text, /stats/);
  assert.match((await send(42, '/music not-a-link')).data.text, /music|link|unsupported/i);
  const ordinaryMenu = await send(43, '/settings');
  await fixtureSql("UPDATE users SET ban_until=(unixepoch('now')+3600)*1000000 WHERE id=43");
  try {
    for (const route of ['/api/v1/music/me/stats', '/api/v1/stats/groups']) assert.equal((await request(route, ordinary)).status, 403, 'existing JWT must use fresh bans');
    const start = botApi.calls.length;
    for (const command of ['/stats', '/music not-a-link']) botApi.enqueue(43, 43, command);
    // Stats is silent for banned actors; Music replies with a denial. The callback fences both.
    const callback = botApi.enqueueCallback(43, 43, ordinaryMenu.messageId, 'prefs:quality:_menu');
    const ack = await waitFor(() => botApi.calls.slice(start).find(call => call.method === 'answercallbackquery' && call.data.callback_query_id === callback), 'banned same-chat callback denial');
    assert.equal(ack.data.show_alert, true);
    const deliveries = botApi.calls.slice(start).filter(call => Number(call.data.chat_id) === 43 && ['sendmessage', 'sendaudio', 'senddocument', 'sendvideo'].includes(call.method));
    assert.equal(deliveries.length, 1, 'banned commands cannot return Stats data or media');
    assert.equal(deliveries[0].method, 'sendmessage');
    assert.match(deliveries[0].data.text, /^Music access denied\.?$/);
  } finally { await fixtureSql('UPDATE users SET ban_until=NULL WHERE id=43'); }
  assert.equal((await request('/api/v1/music/me/stats', ordinary)).status, 200, 'existing JWT sees restored authority');
  assert.match((await send(43, '/stats')).data.text, /stats/);
  await fixtureSql("UPDATE users SET role='restricted' WHERE id=43");
  try {
    assert.equal((await request('/api/v1/stats/groups', ordinary)).status, 403, 'Stats must reload revoked role from the shared store');
    const start = botApi.calls.length;
    botApi.enqueue(43, 43, '/stats');
    const callback = botApi.enqueueCallback(43, 43, ordinaryMenu.messageId, 'prefs:quality:_menu');
    const ack = await waitFor(() => botApi.calls.slice(start).find(call => call.method === 'answercallbackquery' && call.data.callback_query_id === callback), 'restricted same-chat callback denial');
    assert.equal(ack.data.show_alert, true);
    assert(!botApi.calls.slice(start).some(call => call.method === 'sendmessage' && Number(call.data.chat_id) === 43), 'revoked Stats command must not return data');
  } finally { await fixtureSql("UPDATE users SET role='user' WHERE id=43"); }
  const sessionSummary = await runUserSessionScenarios({ request, fixture: context.userSessionFixture, owner, ordinary, fixtureSql, waitFor, context: { botApi } });
  assert.equal((await request('/api/v1/stats/import/by-path', ordinary, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status, 403);

  const rawChat = -8000000000011;
  const rawMessage = 990001;
  let rawUpdate;
  let acknowledged = false;
  botApi.beforePollAcknowledgement = async offset => {
    if (rawUpdate !== undefined && offset > rawUpdate && !acknowledged) {
      assert.equal(await fixtureSql(`SELECT text FROM stats_messages WHERE chat_id=${rawChat} AND message_id=${rawMessage}`), 'native raw persistence boundary');
      acknowledged = true;
    }
  };
  try {
    rawUpdate = botApi.enqueue(42, rawChat, 'native raw persistence boundary', rawMessage);
    await waitFor(() => acknowledged, 'raw Stats row committed before offset acknowledgement');
  } finally { botApi.beforePollAcknowledgement = undefined; }

  const held = [holdImportBody(base, owner), holdImportBody(base, owner)];
  try {
    await waitFor(async () => (await request('/api/v1/stats/import/by-path', owner, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status === 503, 'both real HTTP import handlers own capacity');
    held[1].close();
    await held[1].done;
    const body = new FormData();
    body.append('file', new Blob([desktopExport()], { type: 'application/json' }), 'desktop.json');
    // Wait for the second canceled client's capacity to be released, not a guessed sleep.
    await waitFor(async () => (await request('/api/v1/stats/import/by-path', owner, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' })).status === 400, 'second held request drained');
    const submitted = await request(`/api/v1/stats/import?chat_id=${desktopChat}`, owner, { method: 'POST', body });
    assert.equal(submitted.status, 200);
    const job = await request(`/api/v1/stats/import/${submitted.data.job_id}`, owner);
    assert.equal(job.status, 200);
    assert.equal(job.data.status, 'running', 'bounded actual Desktop job must be observed running before TERM');
    const proof = await stopProducts();
    const canceled = await held[0].done;
    assert(canceled.error || canceled.status === 503, 'held real HTTP import must cancel on shutdown');
    console.log(`Native products original process proof: ${JSON.stringify(proof)}`);
    await reopenProducts();
    const rows = await fixtureSql(`SELECT count(*) FROM stats_messages WHERE chat_id=${desktopChat}`);
    assertAtomicRows(rows, desktopMessages);
    assert.equal(await fixtureSql(`SELECT count(*) FROM stats_messages WHERE chat_id=${desktopChat} AND (message_id<1 OR message_id>${desktopMessages} OR from_user<>42 OR text<>'native atomic import '||(message_id-1))`), '0', 'reopened import rows must match the submitted export');
    assert.equal(await fixtureSql(`SELECT text FROM stats_messages WHERE chat_id=${rawChat} AND message_id=${rawMessage}`), 'native raw persistence boundary');
    assert.equal((await request('/health/ready')).status, 200);
    return `actual Music/Stats factories, raw-before-offset, fresh bans, HTTP cancellation and Desktop running job TERM/reopen atomic rows=${rows}/${desktopMessages}; ${sessionSummary}`;
  } finally {
    for (const item of held) item.close();
    await Promise.all(held.map(item => item.done));
  }
}
