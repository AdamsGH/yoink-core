import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import dgram from 'node:dgram';
import { isIPv4 } from 'node:net';
import { waitFor } from './helpers.mjs';

export function bridgeAddress(value) {
  assert(typeof value === 'string' && /^(?:\d{1,3}\.){3}\d{1,3}$/.test(value), 'DNS helper needs an allocated IPv4 address on existing Docker bridge');
  const bytes = value.split('.').map(Number);
  assert(bytes.every(byte => byte <= 255) && bytes.join('.') === value && (bytes[0] === 10 || (bytes[0] === 172 && bytes[1] >= 16 && bytes[1] <= 31) || (bytes[0] === 192 && bytes[1] === 168) || (bytes[0] === 100 && bytes[1] >= 64 && bytes[1] <= 127)), 'DNS helper must use an allocated private or shared Docker bridge address; no public/host listener');
  return value;
}
export function resolverArgs(address) {
  return ['--dns', bridgeAddress(address), '--dns-search', '.', '--dns-option', 'timeout:30', '--dns-option', 'attempts:2'];
}
export function heldWorkerEvidence(snapshot, state, hostname) {
  assert.equal(state.phase, 'holding');
  assert.deepEqual(snapshot.errors, [], 'read-only /proc visibility required, never add capabilities to the native server');
  for (const worker of snapshot.workers) {
    if (!worker.sockets.some(socket => state.records.some(record => record.name === hostname && record.held && !record.replied && record.port === socket.port && record.transport === socket.transport && (record.source === socket.source || socket.source === '0.0.0.0')))) continue;
    assert(Number.isSafeInteger(worker.pid) && worker.pid > 1 && snapshot.children.includes(worker.pid));
    assert.equal(worker.parent, 1);
    assert.deepEqual(worker.argv, ['/usr/local/bin/python', '-I', '-B', '/opt/yoink-worker/main.py']);
    assert(worker.cwd.startsWith('/state/inbox/') && !worker.cwd.includes('/../'));
    assert.equal(worker.mode, 0o700);
    assert.deepEqual(worker.uid, ['10001', '10001', '10001', '10001']);
    assert.equal(worker.noNewPrivs, '1');
    assert(/^0+$/.test(worker.capEff), 'actual worker retains zero capabilities');
    assert.deepEqual(JSON.parse(worker.privateOrigins), [], 'no private-origin exemption');
    assert.deepEqual(JSON.parse(worker.proxyOrigins), [], 'no proxy-origin exemption');
    return worker;
  }
  return null;
}

export async function probeDns(address, hostname, { port = 53, timeout = 1500 } = {}) {
  assert(isIPv4(address) && Number.isInteger(port) && port > 0 && port <= 65535);
  assert(Number.isInteger(timeout) && timeout > 0 && timeout <= 1500);
  const labels = hostname.split('.');
  assert(hostname.length < 253 && labels.every(label => /^[a-z0-9-]{1,63}$/.test(label)));
  const question = Buffer.concat([...labels.map(label => Buffer.concat([Buffer.from([label.length]), Buffer.from(label)])), Buffer.from([0, 0, 1, 0, 1])]);
  const header = Buffer.alloc(12), ident = randomBytes(2).readUInt16BE();
  header.writeUInt16BE(ident, 0); header.writeUInt16BE(0x0100, 2); header.writeUInt16BE(1, 4);
  const socket = dgram.createSocket('udp4');
  await new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => finish(new Error('owned DNS helper UDP self-test deadline')), timeout);
    function finish(error) {
      if (settled) return; settled = true; clearTimeout(timer);
      try { socket.close(); } catch { /* unbound error path has no socket FD */ }
      if (error) reject(error); else resolve();
    }
    socket.once('error', finish);
    socket.on('message', (packet, peer) => {
      if (peer.address !== address || peer.port !== port || packet.length < 12 || packet.readUInt16BE(0) !== ident) return;
      try {
        assert.equal(packet.readUInt16BE(2), 0x8503, 'authoritative NXDOMAIN; never address answers');
        assert.deepEqual([...Array(4)].map((_, index) => packet.readUInt16BE(4 + index * 2)), [1, 0, 0, 0]);
        assert.equal(packet.length, 12 + question.length); assert(packet.subarray(12).equals(question));
        finish();
      } catch (error) { finish(error); }
    });
    socket.send(Buffer.concat([header, question]), port, address, error => { if (error) finish(error); });
  });
}

export async function createOwnedDnsContainer({ docker, containers }, args) {
  // Track the created ID before starting: runc/sysctl failure must not strand a
  // named helper that never returned from `docker run --detach`.
  const output = await docker(['create', '--pull', 'never', ...args]);
  const ids = output.split(/\r?\n/).filter(line => /^[a-f0-9]{64}$/i.test(line));
  assert.equal(ids.length, 1, 'owned DNS docker create must return one container ID');
  const id = ids[0]; containers.add(id);
  try { await docker(['start', id]); return id; }
  catch (error) {
    try { await docker(['rm', '--force', id]); containers.delete(id); }
    catch { /* retained in owned set for outer finally cleanup retry */ }
    throw error;
  }
}

export async function startHeldDns({ startContainer, docker, image, runId, poll = waitFor, fetchImpl = fetch, probe = probeDns, removeContainer = id => docker(['rm', '--force', id]) }) {
  assert(/^[a-f0-9]{16}$/.test(runId));
  const hostname = `inbox-${runId}.example.com`, token = randomBytes(24).toString('hex');
  const source = await readFile(new URL('./held-dns.py', import.meta.url), 'utf8');
  const container = await startContainer(['--name', `yoink-inbox-dns-${runId}`, '--network', 'bridge', '--user', '10001:10001', '--workdir', '/', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--sysctl', 'net.ipv4.ip_unprivileged_port_start=0', '--pids-limit', '32', '--memory', '64m', '--entrypoint', '/usr/local/bin/python', image, '-I', '-B', '-c', source, hostname, token]);
  let closed = false;
  const close = async () => {
    if (closed) return;
    await removeContainer(container); closed = true;
  };
  try {
    const address = bridgeAddress(await docker(['inspect', '--format', '{{.NetworkSettings.Networks.bridge.IPAddress}}', container]));
    const control = async (path, method = 'GET') => {
      assert(['/state', '/release'].includes(path));
      const response = await fetchImpl(`http://${address}:8053${path}`, { method, headers: { 'X-Fixture-Token': token }, signal: AbortSignal.timeout(1500), redirect: 'error' });
      assert.equal(response.status, 200, 'owned DNS helper control status');
      // Same bounded reader as native HTTP helper, but injectable for unit tests.
      let size = 0;
      const chunks = [];
      for await (const chunk of response.body) {
        size += chunk.length; assert(size <= 65536, 'DNS helper response bound'); chunks.push(chunk);
      }
      const state = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      assert(['holding', 'denying'].includes(state.phase) && Array.isArray(state.records) && state.records.length <= 64);
      return state;
    };
    await poll(async () => {
      try { return (await control('/state')).phase === 'holding'; }
      catch (error) {
        if (await docker(['inspect', '--format', '{{.State.Running}}', container]) !== 'true') throw new Error(`DNS helper failed to bind inside its existing bridge namespace: ${await docker(['logs', container])}`, { cause: error });
        return false;
      }
    }, 'owned DNS helper readiness on existing Docker bridge', 10000);
    // Unknown probe name receives NXDOMAIN immediately and is not a held record.
    // The real admission's DNS query must still belong to its observed child FD.
    await probe(address, `probe.${hostname}`);
    return { hostname, address, container, state: () => control('/state'), release: () => control('/release', 'POST'), close };
  } catch (error) {
    await close();
    throw error;
  }
}
