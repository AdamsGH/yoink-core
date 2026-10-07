import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readdir, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { once } from 'node:events';
import http from 'node:http';
import { createApp } from '../index.js';
import { config, job } from '../lifecycle.mjs';
import { Innertube, Platform } from 'youtubei.js';

const tokens = { access_token: 'synthetic-secret', refresh_token: 'synthetic-refresh' };
const body = { url: 'abcdefghijk', tokens, audio_only: true };
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function fixture(fn, overrides = {}, session) {
  const dir = await mkdtemp(join(tmpdir(), 'youtubei-test-'));
  const settings = { ...config(), concurrency: 1, deadlineMs: 100, ...overrides };
  const factory = session || (async () => ({ getBasicInfo: async () => ({ basic_info: { title: 'Fixture', duration: 1 }, download: async () => new ReadableStream({ start(c) { c.enqueue(Buffer.from('media')); c.close(); } }) }) }));
  const server = createApp({ settings, downloadDir: dir, session: factory }).listen(0, '127.0.0.1');
  await once(server, 'listening');
  const url = `http://127.0.0.1:${server.address().port}`;
  const call = (path = '/download', payload = body) => fetch(url + path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload) });
  try { await fn({ dir, url, call, settings }); }
  finally { server.closeAllConnections(); await new Promise(r => server.close(r)); await rm(dir, { recursive: true, force: true }); }
}

test('configuration fails closed, including nonzero queue and malformed deadlines', () => {
  for (const env of [{ JOB_DEADLINE_MS: '0' }, { JOB_DEADLINE_MS: '1e3' }, { MAX_QUEUE: '1' }, { MAX_CONCURRENCY: '17' }, { MAX_FILE_BYTES: '-1' }]) assert.throws(() => config(env));
});
test('pinned public SDK exports and locally generated session use the configured fetch API', async () => {
  assert.equal(typeof Platform.shim.eval, 'function');
  assert.deepEqual(JSON.parse(JSON.stringify(await Platform.shim.eval({ output: 'return { sig: sig, n: n };' }, { sig: 'fixture-sig', n: 'fixture-n' }))), { sig: 'fixture-sig', n: 'fixture-n' });
  let calls = 0;
  const yt = await Innertube.create({ generate_session_locally: true, retrieve_player: false, retrieve_innertube_config: false, fetch: async () => { calls++; throw new Error('network forbidden'); } });
  assert.equal(typeof yt.session.signIn, 'function');
  assert.equal(typeof yt.getBasicInfo, 'function');
  assert.equal(calls, 0);
  await assert.rejects(Platform.shim.eval({ output: 'while (true) {}' }, {}), /timed out/);
});
test('legacy GET-info, POST-info, streaming download and validation slot release', async () => fixture(async ({ url, call, dir }) => {
  for (const payload of [{}, { ...body, url: 2 }]) assert.equal((await call('/download', payload)).status, 400);
  assert.equal((await call('/info', {})).status, 400);
  assert.equal((await call('/info')).status, 200);
  assert.equal((await fetch(url + '/info?' + new URLSearchParams({ url: body.url, ...tokens }))).status, 200);
  const response = await call(); assert.equal(await response.text(), 'media');
  await sleep(20); assert.deepEqual(await readdir(dir), []);
}));
test('refresh rejection and secret/path-redacted errors', async () => {
  await fixture(async ({ call }) => assert.equal((await call('/info')).status, 409), {}, async (_, update) => { update(tokens); return {}; });
  await fixture(async ({ call }) => {
    const response = await call(); assert.equal(response.status, 500);
    assert.equal(await response.text(), '{"error":"download failed"}');
  }, {}, async () => { throw new Error('synthetic-secret /remote/private/file'); });
});
test('file and aggregate scratch ceilings clean intermediates', async () => {
  for (const limits of [{ maxFileBytes: 3 }, { maxScratchBytes: 3 }]) await fixture(async ({ call, dir }) => {
    assert.equal((await call()).status, 500); await sleep(20); assert.deepEqual(await readdir(dir), []);
  }, limits);
});
test('concurrency zero-queue rejection and deadline release', async () => fixture(async ({ call, dir }) => {
  const first = call(); await sleep(20);
  assert.equal((await call()).status, 503);
  assert.equal((await first).status, 500);
  assert.deepEqual(await readdir(dir), []);
  assert.notEqual((await call('/info', {})).status, 503);
}, {}, async () => new Promise(() => {})));
test('disconnect cancels upstream stream and releases scratch and admission slot', async () => {
  let cancelled = false;
  await fixture(async ({ url, call, dir }) => {
    const request = http.request(url + '/download', { method: 'POST', headers: { 'Content-Type': 'application/json' } });
    request.on('error', () => {}); request.end(JSON.stringify(body));
    await sleep(40); request.destroy();
    for (let n = 0; n < 30 && !cancelled; n++) await sleep(10);
    assert.equal(cancelled, true); await sleep(20);
    assert.deepEqual(await readdir(dir), []);
    assert.equal((await call('/info', {})).status, 400);
  }, { deadlineMs: 2000 }, async () => ({ getBasicInfo: async () => ({ basic_info: { title: 'Fixture' }, download: async () => new ReadableStream({ cancel() { cancelled = true; } }) }) }));
});
test('stream writer handles backpressure and never buffers whole input', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'youtubei-stream-')); const ctx = job({ ...config(), deadlineMs: 2000 });
  try {
    let count = 0;
    await ctx.streamToFile(new ReadableStream({ pull(c) { if (count++ === 100) c.close(); else c.enqueue(Buffer.alloc(1024, 1)); } }), join(dir, 'file'));
    assert.equal((await readFile(join(dir, 'file'))).length, 102400);
  } finally { ctx.dispose(); await rm(dir, { recursive: true }); }
});
