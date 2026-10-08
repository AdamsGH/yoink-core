import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import { options, waitFor, initData, literal, cleanup, report, freePort, api } from './helpers.mjs';
import { TrustedServices, receiptProxy } from './services.mjs';
import { BotApiFixture } from '../media-fixture/bot-api.mjs';

const image = 'sha256:' + 'a'.repeat(64);
const args = ['--server-image', image, '--fixture-image', image, '--operator-image', image];
test('immutable supplied images, optional browser inputs', () => {
  assert.equal(options(args)['worker-image'], image);
  assert.equal(options([...args, '--frontend-image', image, '--chromium', '/usr/bin/chromium'])['server-image'], image);
  assert.equal(options(['--server-image', `registry.local:5000/fixture@${image}`, ...args.slice(2)])['server-image'], `registry.local:5000/fixture@${image}`);
  assert.throws(() => options(['--server-image', `-option@${image}`, ...args.slice(2)]));
});
for (const [label, input] of [
  ['missing images', []], ['mutable tag', ['--server-image', 'latest', ...args.slice(2)]],
  ['unknown option', [...args, '--database', '/production.db']], ['duplicate option', [...args, '--server-image', image]],
  ['missing value', [...args, '--chromium']], ['option as value', [...args, '--chromium', '--server-image']],
  ['malformed digest', ['--server-image', 'sha256:123', ...args.slice(2)]],
  ['relative chromium', [...args, '--chromium', 'chromium']],
  ['missing fixture image', [...args.slice(0, 2), ...args.slice(4)]],
  ['unknown scenario', [...args, '--scenarios', 'pretend']],
  ['duplicate scenario', [...args, '--scenarios', 'routing,routing']],
  ['empty scenario entry', [...args, '--scenarios', 'routing,']],
]) test(`reject ${label}`, () => assert.throws(() => options(input)));
test('explicit independent scenario selection and optional importer image', () => {
  const value = options([...args, '--scenarios', 'routing,deadlineContracts', '--importer-image', image]);
  assert.equal(value.scenarios, 'routing,deadlineContracts');
  assert.equal(value['importer-image'], image);
});
test('session init data deterministic and actor validated', () => {
  assert.equal(initData(42, 'fixture', 1700000000), initData(42, 'fixture', 1700000000));
  assert.notEqual(initData(42, 'fixture', 1700000000), initData(43, 'fixture', 1700000000));
  assert.throws(() => initData(-1, 'fixture'));
  assert.throws(() => initData(42, ''));
});
test('SQL quoting never treats names as syntax', () => assert.equal(literal("a' ; --"), "'a'' ; --'"));
test('polling returns evidence and reports bounded timeout', async () => {
  let calls = 0;
  assert.equal(await waitFor(() => ++calls === 2 ? 'evidence' : false, 'poll', 500), 'evidence');
  await assert.rejects(waitFor(() => false, 'missing evidence', 1), /deadline: missing evidence/);
});
test('poll input bounds reject unbounded deadlines', async () => {
  for (const timeout of [0, -1, NaN, Infinity, 180001]) await assert.rejects(waitFor(() => true, 'poll', timeout));
  await assert.rejects(waitFor(null, 'poll'));
});
test('poll failures are not silently retried as readiness', async () => {
  await assert.rejects(waitFor(() => { throw Error('DB failure'); }, 'poll'), /DB failure/);
});
test('cleanup LIFO attempts all resources despite failure', async () => {
  const seen = [];
  await assert.rejects(cleanup([async () => seen.push(1), async () => { seen.push(2); throw Error('failure'); }, async () => seen.push(3)]), AggregateError);
  assert.deepEqual(seen, [3, 2, 1]);
});
test('cleanup success awaits asynchronous resources', async () => {
  const seen = []; await cleanup([async () => seen.push('a'), async () => seen.push('b')]); assert.deepEqual(seen, ['b', 'a']);
});
test('failure report bounded and redacted', () => {
  assert(!report(new Error('secret token'), ['secret', 'token']).includes('secret'));
  assert.equal(report('x'.repeat(30000)).length, 12000);
});
test('free loopback port valid', async () => assert((await freePort()) > 1024));

test('trusted GitHub README feeds real adapter-compatible data', async () => {
  const service = await new TrustedServices().start();
  try {
    const repo = await api(service.origin)('/repos/fixture/repo'); assert.equal(repo.full_name, 'fixture/repo');
    const rd = await api(service.origin)('/repos/fixture/repo/readme'); assert.match(Buffer.from(rd.content, 'base64').toString(), /README/);
    await api(service.origin)('/not-supported', { expected: 404 });
  } finally { await service.close(); }
});
test('gateway completion is deterministic OpenAI wire JSON', async () => {
  const service = await new TrustedServices().start();
  try {
    const value = await api(service.origin)('/v1/chat/completions', { method: 'POST', body: { messages: [{ role: 'user', content: 'classify' }] } });
    assert.equal(JSON.parse(value.choices[0].message.content).summary, 'Native fixture classification.');
    await assert.rejects(api(service.origin)('/v1/chat/completions', { method: 'POST', body: {} }));
  } finally { await service.close(); }
});
test('lost create receipt retains remote candidate without fake native state', async () => {
  const service = await new TrustedServices().start();
  try {
    service.dropCreate = true;
    const input = { query: 'mutation { createUserList }', variables: { input: { name: 'Lost', description: '' } } };
    await assert.rejects(api(service.origin)('/graphql', { method: 'POST', body: input }));
    assert.equal(service.lists.length, 1);
    const result = await api(service.origin)('/graphql', { method: 'POST', body: { query: 'query { user(login: "fixture") { lists } }', variables: {} } });
    assert.equal(result.data.user.lists.nodes[0].name, 'Lost');
  } finally { await service.close(); }
});
test('receipt proxy consumes real send response then drops once', async () => {
  const bot = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
  const port = await bot.start(); const proxy = await receiptProxy(`http://127.0.0.1:${port}`);
  try {
    const request = api(proxy.origin); proxy.dropSend = true;
    await assert.rejects(request('/bot123456:fixture/sendMessage', { method: 'POST', body: { chat_id: 42, text: 'once' } }));
    assert.equal(bot.calls.filter(x => x.method === 'sendmessage').length, 1);
    const sent = await request('/bot123456:fixture/sendMessage', { method: 'POST', body: { chat_id: 42, text: 'second' } });
    assert.equal(sent.ok, true); assert.equal(sent.result.from.id, 123456);
  } finally { await proxy.close(); await bot.close(); }
});
test('receipt proxy drops real edit but fake retains effect', async () => {
  const bot = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
  const port = await bot.start(); const proxy = await receiptProxy(`http://127.0.0.1:${port}`);
  try {
    const request = api(proxy.origin);
    const sent = await request('/bot123456:fixture/sendMessage', { method: 'POST', body: { chat_id: 42, text: 'initial' } });
    proxy.dropEdit = true;
    await assert.rejects(request('/bot123456:fixture/editMessageText', { method: 'POST', body: { chat_id: 42, message_id: sent.result.message_id, text: 'complete' } }));
    assert.equal(bot.progressMessages[0].text, 'complete');
    assert.equal(proxy.calls.filter(x => x.drop).length, 1);
  } finally { await proxy.close(); await bot.close(); }
});
test('HTTP helper caps streamed responses before JSON parsing', async () => {
  const server = http.createServer((req, res) => { res.end('x'.repeat(1024 * 1024 + 1)); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try { await assert.rejects(api(`http://127.0.0.1:${server.address().port}`)('/large'), /exceeds fixture bound/); }
  finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});
test('unexpected HTTP status never prints issued credentials', async () => {
  const secret='issued-fixture-credential';
  const server=http.createServer((req,res)=>{res.writeHead(500);res.end(JSON.stringify({access_token:secret,api_key:secret}));});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try {await assert.rejects(api(`http://127.0.0.1:${server.address().port}`)('/api/v1/auth/token'),error=>{assert(!error.message.includes(secret));return true;});}
  finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
});
test('trusted service rejects oversized request bodies', async () => {
  const service = await new TrustedServices().start();
  try { await assert.rejects(api(service.origin)('/v1/chat/completions', { method: 'POST', body: { messages: [], padding: 'x'.repeat(128 * 1024) } })); }
  finally { await service.close(); }
});
test('paginated deadline fake completes first read and records cancelled second read', async () => {
  const service = await new TrustedServices().start(); service.deadlineStars = true;
  try {
    const first = await fetch(service.origin + '/user/starred');
    assert.match(first.headers.get('link'), /page=2/);
    assert.deepEqual(await first.json(), []);
    await assert.rejects(fetch(service.origin + '/user/starred?page=2', { signal: AbortSignal.timeout(200) }));
    await waitFor(() => service.deadlineRequests.some(row => row.page === '2' && row.closedAfter !== null), 'cancelled page');
    assert(service.deadlineRequests.find(row => row.page === '2').closedAfter < 1000);
  } finally { await service.close(); }
});
test('trusted held request cleanup closes admitted sockets', async () => {
  const service = await new TrustedServices().start(); service.holdRepository = true;
  const pending = fetch(service.origin + '/repos/fixture/held').then(() => false, () => true);
  await waitFor(() => service.held === 1, 'held service'); await service.close(); assert.equal(await pending, true);
});
