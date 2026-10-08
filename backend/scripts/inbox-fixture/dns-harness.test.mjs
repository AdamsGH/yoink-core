import test from 'node:test';
import assert from 'node:assert/strict';
import dgram from 'node:dgram';
import { bridgeAddress, resolverArgs, heldWorkerEvidence, startHeldDns, probeDns, createOwnedDnsContainer } from './dns-harness.mjs';

const hostname = 'inbox-0123456789abcdef.example.com';
function evidence() {
  return {
    snapshot: { children: [81], errors: [], workers: [{ pid: 81, parent: 1, argv: ['/usr/local/bin/python', '-I', '-B', '/opt/yoink-worker/main.py'], cwd: '/state/inbox/actual-child-directory', mode: 0o700, uid: Array(4).fill('10001'), noNewPrivs: '1', capEff: '0000000000000000', privateOrigins: '[]', proxyOrigins: '[]', sockets: [{ source: '172.17.0.1', port: 43123, transport: 'udp', inode: '997' }] }] },
    state: { phase: 'holding', records: [{ name: hostname, source: '172.17.0.1', port: 43123, type: 1, transport: 'udp', held: true, replied: false }] },
  };
}
test('DNS helper accepts only allocated private bridge IPv4 addresses', () => {
  for (const address of ['10.0.2.3', '172.17.0.2', '192.168.5.8', '100.64.0.2', '100.127.255.254']) assert.equal(bridgeAddress(address), address);
});
for (const address of ['93.184.216.34', '100.63.255.254', '100.128.0.2', '127.0.0.1', '0.0.0.0', '172.17.0.2:53', '::1', '172.017.0.2', '172.17.0.256']) test(`reject unsafe DNS helper address ${address}`, () => assert.throws(() => bridgeAddress(address)));
test('resolver override has one numeric DNS with root search and bounded hold window', () => {
  assert.deepEqual(resolverArgs('172.17.0.2'), ['--dns', '172.17.0.2', '--dns-search', '.', '--dns-option', 'timeout:30', '--dns-option', 'attempts:2']);
});
test('actual child/socket metadata and unanswered query must correlate', () => {
  const { snapshot, state } = evidence();
  assert.equal(heldWorkerEvidence(snapshot, state, hostname).pid, 81);
  snapshot.workers[0].sockets[0].source = '0.0.0.0';
  assert.equal(heldWorkerEvidence(snapshot, state, hostname).pid, 81, 'unconnected UDP wildcard still needs matching owned port');
});
test('socket with different source port cannot witness held query', () => {
  const { snapshot, state } = evidence(); state.records[0].port++;
  assert.equal(heldWorkerEvidence(snapshot, state, hostname), null);
});
test('already replied query cannot witness admission', () => {
  const { snapshot, state } = evidence(); state.records[0].replied = true;
  assert.equal(heldWorkerEvidence(snapshot, state, hostname), null);
});
test('query for different public hostname cannot witness admission', () => {
  const { snapshot, state } = evidence(); state.records[0].name = 'other.example.com';
  assert.equal(heldWorkerEvidence(snapshot, state, hostname), null);
});
for (const [label, mutate] of [
  ['unowned child', snapshot => { snapshot.children = []; }],
  ['wrong worker argv', snapshot => { snapshot.workers[0].argv = ['/usr/local/bin/python', 'stub.py']; }],
  ['private exception', snapshot => { snapshot.workers[0].privateOrigins = '["http://127.0.0.1"]'; }],
  ['proxy exception', snapshot => { snapshot.workers[0].proxyOrigins = '["http://127.0.0.1"]'; }],
  ['privilege widening', snapshot => { snapshot.workers[0].capEff = '0000000000000001'; }],
  ['missing no-new-privileges', snapshot => { snapshot.workers[0].noNewPrivs = '0'; }],
  ['wrong workspace mode', snapshot => { snapshot.workers[0].mode = 0o755; }],
  ['proc access blocker', snapshot => { snapshot.errors = ['read-only /proc observation denied']; }],
]) test(`reject live-worker evidence with ${label}`, () => {
  const { snapshot, state } = evidence(); mutate(snapshot);
  assert.throws(() => heldWorkerEvidence(snapshot, state, hostname));
});

function mockHelper({ address = '172.17.0.2', payload } = {}) {
  const starts = [], removals = [], requests = [], image = 'sha256:' + 'a'.repeat(64);
  let oversized = false;
  return {
    starts, removals, requests, image, oversize: () => { oversized = true; },
    args: {
      image, runId: '0123456789abcdef',
      startContainer: async args => { starts.push(args); return 'b'.repeat(64); },
      docker: async args => args[0] === 'inspect' ? address : '',
      removeContainer: async id => { removals.push(id); },
      poll: async fn => { assert(await fn()); },
      probe: async (address, name) => { assert.equal(address, '172.17.0.2'); assert.equal(name, `probe.${hostname}`); },
      fetchImpl: async (url, options) => {
        requests.push({ url, options });
        return new Response(oversized ? 'x'.repeat(65537) : JSON.stringify(payload ?? { phase: options.method === 'POST' ? 'denying' : 'holding', records: [] }));
      },
    },
  };
}
test('DNS helper is contained on existing bridge, no published port or server permission changes', async () => {
  const mock = mockHelper(), dns = await startHeldDns(mock.args);
  try {
    const args = mock.starts[0];
    assert.equal(args[args.indexOf('--network') + 1], 'bridge');
    assert.equal(args[args.indexOf('--user') + 1], '10001:10001');
    assert.equal(args[args.indexOf('--cap-drop') + 1], 'ALL');
    assert(args.includes('no-new-privileges'));
    assert(args.includes('net.ipv4.ip_unprivileged_port_start=0'));
    assert.equal(args[args.indexOf('--entrypoint') + 1], '/usr/local/bin/python');
    for (const forbidden of ['--publish', '-p', '--cap-add', '--privileged', '--mount', '--volume']) assert(!args.includes(forbidden));
    assert(args.includes(mock.image));
    assert.equal(dns.hostname, hostname);
    assert.equal((await dns.release()).phase, 'denying');
    assert(mock.requests.every(({ url, options }) => url.startsWith('http://172.17.0.2:8053/') && options.redirect === 'error' && /^[a-f0-9]{48}$/.test(options.headers['X-Fixture-Token'])));
  } finally { await dns.close(); }
});
test('DNS helper close is tracked and idempotent', async () => {
  const mock = mockHelper(), dns = await startHeldDns(mock.args);
  await dns.close(); await dns.close(); assert.equal(mock.removals.length, 1);
});
test('DNS helper unsafe allocation fails closed and cleans its owned container', async () => {
  const mock = mockHelper({ address: '127.0.0.1' });
  await assert.rejects(startHeldDns(mock.args), /private or shared Docker bridge/);
  assert.equal(mock.removals.length, 1); assert.equal(mock.requests.length, 0);
});
test('DNS helper control responses remain bounded', async () => {
  const mock = mockHelper(), dns = await startHeldDns(mock.args);
  try { mock.oversize(); await assert.rejects(dns.state(), /response bound/); }
  finally { await dns.close(); }
});

async function udpFixture(reply, run) {
  const server = dgram.createSocket('udp4');
  server.on('message', (packet, peer) => { const response = reply(packet); if (response) server.send(response, peer.port, peer.address); });
  await new Promise(resolve => server.bind(0, '127.0.0.1', resolve));
  try { await run(server.address().port); }
  finally { await new Promise(resolve => server.close(resolve)); }
}
test('DNS wire self-test requires authoritative NXDOMAIN with zero address records', async () => {
  await udpFixture(packet => { const response = Buffer.from(packet); response.writeUInt16BE(0x8503, 2); return response; }, async port => {
    await probeDns('127.0.0.1', 'probe.example.com', { port });
  });
});
test('DNS wire self-test rejects non-NXDOMAIN replies', async () => {
  await udpFixture(packet => { const response = Buffer.from(packet); response.writeUInt16BE(0x8180, 2); return response; }, async port => {
    await assert.rejects(probeDns('127.0.0.1', 'probe.example.com', { port }), /NXDOMAIN/);
  });
});
test('DNS wire self-test times out and closes its owned UDP socket', async () => {
  await udpFixture(() => null, async port => {
    await assert.rejects(probeDns('127.0.0.1', 'probe.example.com', { port, timeout: 50 }), /self-test deadline/);
  });
});
test('DNS wire self-test accepts only numeric bounded destinations and valid labels', async () => {
  await assert.rejects(probeDns('example.com', 'probe.example.com'));
  await assert.rejects(probeDns('127.0.0.1', 'bad..example.com'));
  await assert.rejects(probeDns('127.0.0.1', 'probe.example.com', { port: 0 }));
  await assert.rejects(probeDns('127.0.0.1', 'probe.example.com', { timeout: 0 }));
});
test('failed DNS wire self-test removes the helper before native admission', async () => {
  const mock = mockHelper(); mock.args.probe = async () => { throw new Error('wire self-test rejected'); };
  await assert.rejects(startHeldDns(mock.args), /wire self-test rejected/);
  assert.equal(mock.removals.length, 1);
});

test('DNS helper is tracked before Docker start can fail', async () => {
  const containers = new Set(), id = 'c'.repeat(64), calls = [];
  const docker = async args => {
    calls.push(args[0]);
    if (args[0] === 'create') return `harmless warning\n${id}`;
    if (args[0] === 'start') { assert(containers.has(id)); throw Error('runc sysctl failure'); }
    return '';
  };
  await assert.rejects(createOwnedDnsContainer({ docker, containers }, ['--network', 'bridge']), /runc sysctl failure/);
  assert.deepEqual(calls, ['create', 'start', 'rm']); assert.equal(containers.size, 0);
});
test('DNS helper cleanup failure retains created ID for outer finally retry', async () => {
  const containers = new Set(), id = 'd'.repeat(64);
  const docker = async args => { if (args[0] === 'create') return id; throw Error('Docker unavailable'); };
  await assert.rejects(createOwnedDnsContainer({ docker, containers }, []), /Docker unavailable/);
  assert(containers.has(id));
});
test('DNS helper name/create collision never removes an unowned container', async () => {
  const containers = new Set(), calls = [];
  const docker = async args => { calls.push(args[0]); throw Error('name collision'); };
  await assert.rejects(createOwnedDnsContainer({ docker, containers }, []), /name collision/);
  assert.deepEqual(calls, ['create']); assert.equal(containers.size, 0);
});
