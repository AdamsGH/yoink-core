import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { once } from 'node:events';
import { createSyntheticBgutil } from './youtube-auth-bgutil.mjs';

const payload = { content_binding: 'abcdefghijk' };
async function fixture(t, timeoutMs = 1000) {
  const f = createSyntheticBgutil({ timeoutMs });
  f.server.listen(0, '127.0.0.1');
  await once(f.server, 'listening');
  t.after(() => f.close());
  const origin = `http://127.0.0.1:${f.server.address().port}`;
  const request = async (route, options = {}) => {
    const response = await fetch(origin + route, { signal: AbortSignal.timeout(2000), ...options });
    const body = await response.text();
    return { status: response.status, body: body ? JSON.parse(body) : undefined };
  };
  const control = action => request(`/__fixture/${action}`, { method: 'POST' });
  return { ...f, origin, request, control };
}
const pot = (f, value = payload) => f.request('/get_pot', { method: 'POST', body: JSON.stringify(value) });
async function entered(f, active) {
  for (let i = 0; i < 100; i++) {
    if ((await f.control('state')).body.active === active) return;
    await new Promise(resolve => setTimeout(resolve, 2));
  }
  assert.fail('synthetic job not entered');
}

test('youtube-auth bgutil v2 transport has fixed synthetic binding and token', async t => {
  const f = await fixture(t);
  assert.deepEqual(await f.request('/ping'), { status: 200, body: { version: '2.0.0' } });
  assert.deepEqual(await pot(f), { status: 200, body: { poToken: 'synthetic-pot' } });
  assert.equal((await pot(f, { ...payload, content_binding: 'wrong' })).status, 400);
  assert.equal((await pot(f, { ...payload, proxy: 'http://invalid.example' })).status, 400);
  assert.equal((await f.request('/__fixture/arm', { method: 'POST', body: '{}' })).status, 400);
  assert.equal((await f.control('unknown')).status, 400);
  assert.throws(() => createSyntheticBgutil({ timeoutMs: 90001 }));
});

test('youtube-auth bgutil retains two disconnected jobs until explicit reset, joins cleanup', async t => {
  const f = await fixture(t);
  for (let i = 1; i <= 2; i++) {
    await f.control('arm');
    const req = http.request(f.origin + '/get_pot', { method: 'POST' });
    const done = new Promise(resolve => { req.once('error', resolve); req.once('close', resolve); });
    req.end(JSON.stringify(payload));
    await entered(f, i);
    req.destroy(); await done;
    assert.equal((await f.control('state')).body.active, i);
  }
  assert.equal((await f.control('arm')).status, 409);
  assert.equal((await pot(f)).status, 503);
  await f.control('reset');
  await entered(f, 0);
  assert.equal((await pot(f)).status, 200);
  await f.close();
});

test('youtube-auth bgutil held work has a deadline and explicit release', async t => {
  const f = await fixture(t, 150);
  await f.control('arm');
  assert.equal((await pot(f)).status, 504);
  assert.equal((await f.control('state')).body.active, 0);
  await f.control('arm');
  const held = pot(f);
  await entered(f, 1);
  await f.control('release');
  assert.equal((await held).status, 200);
});
