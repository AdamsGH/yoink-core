import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { once } from 'node:events';
import { createYoutubeAuthGate } from '../../../docker/youtubei-service/test/youtube-auth-gate.mjs';
import { assertCurrentImageHooks, runSyntheticPotGateway } from './youtube-auth-current-image.mjs';
import { assertYoutubeAuthEvidence } from './youtube-auth-scenarios.mjs';

async function fixture(t, timeoutMs = 1000) {
  const gate = createYoutubeAuthGate({ timeoutMs });
  let passed = 0;
  const server = http.createServer((req, res) => {
    gate.handle(req, res, () => { passed++; req.resume(); res.writeHead(200).end('media'); })
      .catch(() => res.destroy());
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(async () => { gate.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); });
  const origin = `http://127.0.0.1:${server.address().port}`;
  const request = async (route, options = {}) => {
    const response = await fetch(origin + route, { method: 'POST', signal: AbortSignal.timeout(2000), ...options });
    return { status: response.status, body: await response.text() };
  };
  const control = (action, route) => request(`/__fixture/${action}${route ? `?route=${route}` : ''}`);
  return { gate, request, control, passed: () => passed };
}

test('youtube-auth current-image requires every coordinator hook, not a partial lane', () => {
  const hooks = Object.fromEntries(['syntheticControl', 'refreshGate', 'restartYoutubeAuth', 'restartYoutubeAuthServices', 'restartYoutubeAuthGateway', 'syntheticPotControl', 'gatewayRequest', 'runSyntheticPotWorker'].map(k => [k, () => {}]));
  assertCurrentImageHooks(hooks);
  for (const key of Object.keys(hooks)) assert.throws(() => assertCurrentImageHooks({ ...hooks, [key]: undefined }), new RegExp(key));
});

test('youtube-auth synthetic gate holds exactly one route and releases without body evidence', async t => {
  const f = await fixture(t);
  assert.equal((await f.control('arm', 'download')).status, 200);
  const held = f.request('/download');
  for (let i = 0; i < 30; i++) {
    if (JSON.parse((await f.control('state')).body).entered) break;
    await new Promise(resolve => setTimeout(resolve, 5));
  }
  assert.deepEqual(JSON.parse((await f.control('state')).body), { armed: true, entered: true });
  assert.equal(f.passed(), 0);
  assert.equal((await f.control('arm', 'info')).status, 409);
  assert.equal((await f.request('/info')).status, 200);
  assert.equal((await f.control('release')).status, 200);
  assert.equal((await held).body, 'media');
  assert.equal(f.passed(), 2);
  assert.deepEqual(JSON.parse((await f.control('state')).body), { armed: false, entered: false });
});

test('youtube-auth synthetic gate deadline and reset fail closed and allow rearm', async t => {
  const f = await fixture(t, 80);
  await f.control('arm', 'download');
  assert.equal((await f.request('/download')).status, 504);
  assert.equal(f.passed(), 0);
  await f.control('arm', 'download');
  const held = f.request('/download');
  while (!JSON.parse((await f.control('state')).body).entered) await new Promise(resolve => setTimeout(resolve, 2));
  await f.control('reset');
  assert.equal((await held).status, 504);
  assert.equal((await f.request('/download')).status, 200);
});

test('youtube-auth synthetic controls reject malformed routes, actions and bodies', async t => {
  const f = await fixture(t);
  for (const route of ['/__fixture/arm?route=pot', '/__fixture/arm?route=download&route=info', '/__fixture/arm?route=download&secret=unused', '/__fixture/nope', '/__fixture/state?route=download']) {
    assert.equal((await f.request(route)).status, 400);
  }
  assert.equal((await f.request('/__fixture/state', { method: 'GET' })).status, 400);
  assert.equal((await f.request('/__fixture/arm?route=download', { body: 'nonempty' })).status, 400);
  assert.deepEqual(JSON.parse((await f.control('state')).body), { armed: false, entered: false });
  assert.throws(() => createYoutubeAuthGate({ timeoutMs: 90001 }));
});

test('youtube-auth synthetic client disconnect releases the only held request', async t => {
  const f = await fixture(t);
  await f.control('arm', 'download');
  const controller = new AbortController();
  const held = f.request('/download', { signal: controller.signal }).catch(() => undefined);
  while (!JSON.parse((await f.control('state')).body).entered) await new Promise(resolve => setTimeout(resolve, 2));
  controller.abort();
  assert.equal(await held, undefined);
  for (let i = 0; i < 100; i++) {
    if (!JSON.parse((await f.control('state')).body).armed) break;
    await new Promise(resolve => setTimeout(resolve, 2));
  }
  assert.equal(f.passed(), 0);
  assert.equal((await f.control('arm', 'info')).status, 200);
});

test('youtube-auth current fixture refresh credentials cannot enter public evidence', () => {
  for (const value of ['yoink-fixture-synthetic-access-not-a-real-token', 'yoink-fixture-synthetic-refresh-not-a-real-token']) {
    assert.throws(() => assertYoutubeAuthEvidence({ text: value }), error => !error.message.includes(value));
  }
});

test('youtube-auth synthetic PoToken orchestration checks retained slots and both restart boundaries', async () => {
  let active = 0, poisoned = 0, gatewayRestarts = 0, pairRestarts = 0, resets = 0;
  await runSyntheticPotGateway({
    gatewayRequest: async (route, payload, { timeoutMs }) => {
      if (timeoutMs === 1000) { active++; poisoned++; throw Error('bounded client disconnect'); }
      if (poisoned === 2) return { status: 503 };
      if (payload.proxy) return { status: 502 };
      if (route === '/pot/ping') return { status: 200, body: { version: '2.0.0' } };
      if (active === 2) { poisoned++; return { status: 502 }; }
      return { status: 200, body: { poToken: 'synthetic-pot' } };
    },
    syntheticPotControl: async action => { if (action === 'reset') resets++; return { active }; },
    runSyntheticPotWorker: async () => true,
    restartYoutubeAuthGateway: async () => { gatewayRestarts++; poisoned = 0; },
    restartYoutubeAuthServices: async () => { pairRestarts++; active = 0; poisoned = 0; },
    waitFor: async predicate => assert(await predicate()),
  });
  assert.equal(gatewayRestarts, 1); assert.equal(pairRestarts, 1); assert.equal(resets, 1);
});

test('youtube-auth synthetic PoToken orchestration rejects a fake provider probe and resets', async () => {
  let resets = 0;
  await assert.rejects(runSyntheticPotGateway({
    gatewayRequest: async route => ({ status: 200, body: route === '/pot/ping' ? { version: '2.0.0' } : { poToken: 'synthetic-pot' } }),
    runSyntheticPotWorker: async () => false,
    syntheticPotControl: async action => { if (action === 'reset') resets++; },
  }));
  assert.equal(resets, 1);
});
