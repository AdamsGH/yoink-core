import assert from 'node:assert/strict';
import test from 'node:test';
import http from 'node:http';
import { spawnSync } from 'node:child_process';
import { setImmediate } from 'node:timers/promises';
import { configuredImages, fingerprintImages } from './immutable-images.mjs';
import { desktopExport, assertAtomicRows, holdImportBody } from './native-products-scenarios.mjs';
import { startUserSessionFixture } from './user-session-scenarios.mjs';
import { BotApiFixture } from './bot-api.mjs';

test('immutable image selection rejects partial, mutable, mixed-source and identical inputs', async () => {
  const runtime = `sha256:${'a'.repeat(64)}`;
  const fixture = `sha256:${'b'.repeat(64)}`;
  const env = { YOINK_MEDIA_RUNTIME_IMAGE: runtime, YOINK_MEDIA_FIXTURE_IMAGE: fixture };
  assert.deepEqual(configuredImages({}, { image: 'old', fixtureImage: 'old-fixture' }, true), { image: 'old', fixtureImage: 'old-fixture', immutable: false });
  for (const invalid of [{ YOINK_MEDIA_RUNTIME_IMAGE: runtime }, { ...env, YOINK_MEDIA_FIXTURE_IMAGE: 'tag:mutable' }, { ...env, YOINK_MEDIA_FIXTURE_IMAGE: runtime }]) assert.throws(() => configuredImages(invalid, {}, false));
  assert.throws(() => configuredImages(env, {}, true));
  const selected = configuredImages(env, {}, false);
  await fingerprintImages(async args => JSON.stringify([{ Id: args[2], Config: { Entrypoint: [args[2] === runtime ? '/usr/local/bin/yoink-server' : '/usr/local/bin/yoink-fixture-server'] } }]), selected);
  await assert.rejects(fingerprintImages(async () => JSON.stringify([{ Id: fixture, Config: {} }]), selected));
});

test('bounded Desktop export has valid unique rows and atomicity rejects partial writes', () => {
  const exported = JSON.parse(desktopExport());
  assert.equal(exported.messages.length, 60000);
  assert.equal(new Set(exported.messages.map(row => row.id)).size, 60000);
  assertAtomicRows('0', 60000);
  assertAtomicRows('60000', 60000);
  assert.throws(() => assertAtomicRows('59999', 60000));
  assert.throws(() => desktopExport(100001));
});

test('held import sends a real incomplete multipart body and joins cancellation', async () => {
  let observed;
  const entered = new Promise(resolve => { observed = resolve; });
  const server = http.createServer((req, res) => {
    assert.equal(req.headers.authorization, 'Bearer fixture-token');
    req.once('data', chunk => observed({ req, res, chunk: chunk.toString() }));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const held = holdImportBody(`http://127.0.0.1:${server.address().port}`, 'fixture-token');
  try {
    const { req, res, chunk } = await entered;
    assert(chunk.includes('filename="held.json"'));
    assert.equal(req.complete, false);
    res.writeHead(503).end();
    assert.deepEqual(await held.done, { status: 503 });
  } finally {
    held.close();
    await held.done;
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test('fake Telegram offset boundary observer runs before acknowledgement is recorded', async () => {
  const bot = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
  const port = await bot.start();
  try {
    const id = bot.enqueue(42, -100, 'raw fixture', 44);
    let observed = false;
    bot.beforePollAcknowledgement = async offset => {
      assert.equal(offset, id + 1);
      assert.equal(bot.polls.length, 0);
      observed = true;
    };
    const result = await fetch(`http://127.0.0.1:${port}/botfixture/getUpdates`, { method: 'POST', body: JSON.stringify({ offset: id + 1 }), signal: AbortSignal.timeout(3000) });
    assert.equal(result.status, 200);
    assert(observed);
    assert.equal(bot.polls[0].offset, id + 1);
  } finally { await bot.close(); }
});

test('disposable tdlight fixture enforces protocol token and rotates it', async () => {
  let installed;
  const secrets = new Set();
  const fixture = await startUserSessionFixture(async token => {
    assert(secrets.has(token), 'register credentials before installing them');
    installed = token;
  }, token => secrets.add(token));
  try {
    const call = token => fetch(`${fixture.origin}/user${token}/getForumTopics`, { method: 'POST', body: 'chat_id=-100', signal: AbortSignal.timeout(3000) });
    assert.equal((await call('wrong')).status, 401);
    const old = installed;
    assert.equal((await call(old)).status, 200);
    await fixture.rotate();
    assert.notEqual(installed, old);
    assert.equal((await call(old)).status, 401);
    assert.equal((await call(installed)).status, 200);
    fixture.setMode('error');
    assert.equal((await call(installed)).status, 502);
    fixture.setMode('ok');
    for (let i = 0; i < 70; i++) assert.equal((await call(installed)).status, 200);
    assert.equal(fixture.calls.length, 64);
    assert.equal(fixture.callCount, 73);
    assert.equal(secrets.size, 2);
    assert(secrets.has(old));
    fixture.assertHealthy();
  } finally { await fixture.close(); }
});

test('failed tdlight token installation closes its listening server', () => {
  const module = new URL('./user-session-scenarios.mjs', import.meta.url).href;
  const child = spawnSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import { startUserSessionFixture } from ${JSON.stringify(module)};
    await assert.rejects(startUserSessionFixture(async () => { throw new Error('install failed'); }), /install failed/);
  `], { encoding: 'utf8', timeout: 3000 });
  assert.ifError(child.error);
  assert.equal(child.status, 0, child.stderr);
});

test('tdlight teardown joins an interrupted request handler without rejection', async () => {
  let installed;
  const fixture = await startUserSessionFixture(async token => { installed = token; });
  const interrupted = new Promise(resolve => {
    const req = http.request(`${fixture.origin}/user${installed}/getForumTopics`, {
      method: 'POST', headers: { 'Content-Length': '4096' },
    });
    req.once('error', resolve);
    req.once('response', () => resolve(new Error('incomplete request unexpectedly completed')));
    req.write('chat_id=-100');
  });
  try {
    const deadline = Date.now() + 3000;
    while (!fixture.pendingRequests && Date.now() < deadline) await setImmediate();
    assert.equal(fixture.pendingRequests, 1);
  } finally { await fixture.close(); }
  assert.equal(fixture.pendingRequests, 0);
  assert.equal((await interrupted).code, 'ECONNRESET');
  fixture.assertHealthy();
});
