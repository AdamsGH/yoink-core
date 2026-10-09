import test from 'node:test';
import assert from 'node:assert/strict';
import { startSyntheticYoutubeAuth, validateYoutubeAuthTopology } from './youtube-auth-topology.mjs';
import { assertYoutubeAuthEvidence, assertYoutubeAuthRequest, assertYoutubeAuthDelivery, runYoutubeAuthScenarios } from './youtube-auth-scenarios.mjs';

const config = () => ({ services: {
  worker: { network_mode: 'none', volumes: [{ type: 'volume', source: 'auth-socket', target: '/run/yoink-youtube-auth', read_only: true }], extra_hosts: { 'youtu.be': '93.184.216.34' } },
  gateway: { networks: { services: {} } },
  youtubei: { networks: { services: {} }, command: ['node', 'test/synthetic-server.mjs'] },
  bgutil: { networks: { services: {} }, command: ['synthetic-pot'] },
}, networks: { services: { internal: true } } });
function hooks({ failUp = false, residual = false, topology = config() } = {}) {
  const calls = [];
  return { calls, capture: async (command, args, options) => {
    calls.push({ command, args, options });
    assert.equal(command, 'docker');
    if (args.includes('config')) return { output: JSON.stringify(topology) };
    if (args.includes('up') && failUp) throw Error('startup failed');
    if (args.includes('ls') && residual) return { output: 'leftover' };
    return { output: '' };
  } };
}
test('isolated no-build/pull topology returns exact hooks and idempotent own cleanup', async () => {
  const h = hooks();
  const fixture = await startSyntheticYoutubeAuth({ capture: h.capture, runId: 'test' });
  assert.deepEqual(Object.keys(fixture).sort(), ['close', 'dockerArgs', 'gatewayRequest', 'restartYoutubeAuthGateway', 'restartYoutubeAuthServices', 'socketVolume', 'syntheticControl', 'syntheticPotControl']);
  assert.equal(fixture.dockerArgs[0], '--mount');
  assert.match(fixture.dockerArgs[1], /^type=volume,source=yoink-auth-test-[a-f0-9]{8}_auth-socket,target=\/run\/yoink-youtube-auth,readonly$/);
  assert.deepEqual(fixture.dockerArgs.slice(2), ['--add-host', 'youtu.be:93.184.216.34']);
  await fixture.close(); await fixture.close();
  const up = h.calls.find(c => c.args.includes('up'));
  assert(up.args.includes('--no-build'));
  assert.deepEqual(up.args.slice(-4), ['--no-build', '--pull', 'never', 'gateway']);
  assert(up.args.includes('docker-compose.youtube-auth.yaml'));
  assert(!up.args.some(a => a.startsWith('backend/')));
  const down = h.calls.filter(c => c.args.includes('down'));
  assert.equal(down.length, 1);
  assert.equal(down[0].options.timeout, 360000);
  const project = up.args[2];
  assert.equal(h.calls.filter(c => c.args.includes(`label=com.docker.compose.project=${project}`)).length, 3);
});
test('startup failure cleans only own compose resources', async () => {
  const h = hooks({ failUp: true });
  await assert.rejects(startSyntheticYoutubeAuth({ capture: h.capture, runId: 'fail' }), /startup failed/);
  assert.equal(h.calls.filter(c => c.args.includes('down')).length, 1);
  assert.equal(h.calls.filter(c => c.args.includes('--filter')).length, 3);
});
test('residual resources fail cleanup rather than being ignored', async () => {
  const h = hooks({ residual: true });
  const fixture = await startSyntheticYoutubeAuth({ capture: h.capture, runId: 'residual' });
  await assert.rejects(fixture.close(), /residual own volumes/);
});
test('unsafe topology and malformed combined output fail before up and clean', async () => {
  for (const mutate of [c => c.services.worker.network_mode = 'bridge', c => c.networks.services.internal = false,
    c => c.services.youtubei.networks.egress = {}, c => c.services.gateway.ports = ['9173:9173'],
    c => c.services.worker.volumes[0].read_only = false, c => c.services.youtubei.command = ['node', 'index.js']]) {
    const c = config(); mutate(c);
    assert.throws(() => validateYoutubeAuthTopology(c));
    const h = hooks({ topology: c });
    await assert.rejects(startSyntheticYoutubeAuth({ capture: h.capture, runId: 'unsafe' }));
    assert(!h.calls.some(c => c.args.includes('up')));
    assert(h.calls.some(c => c.args.includes('down')));
  }
  const h = hooks();
  await assert.rejects(startSyntheticYoutubeAuth({ capture: async (...args) => args[1].includes('config') ? { output: 'warning\n{}' } : h.capture(...args), runId: 'json' }));
  assert(h.calls.some(c => c.args.includes('down')));
});
test('Compose normalized host arrays retain the fixed public mapping', () => {
  const c = config();
  c.services.worker.extra_hosts = ['youtu.be=93.184.216.34'];
  validateYoutubeAuthTopology(c);
  c.services.worker.extra_hosts = ['youtu.be=127.0.0.1'];
  assert.throws(() => validateYoutubeAuthTopology(c));
});
test('persisted/reply evidence rejects synthetic tokens without echoing them', () => {
  assertYoutubeAuthEvidence([{ method: 'sendmessage', data: { text: 'Delivered' } }]);
  for (const token of ['synthetic-access', 'synthetic-refresh', 'synthetic-refresh-observed', 'synthetic-native-access']) {
    assert.throws(() => assertYoutubeAuthEvidence({ data: { text: token } }), error => !error.message.includes(token));
  }
});
test('topology startup errors redact combined-stream synthetic credentials', async () => {
  const h = hooks();
  await assert.rejects(startSyntheticYoutubeAuth({ runId: 'redaction', capture: async (...args) => {
    if (args[1].includes('up')) throw Error('synthetic-access synthetic-refresh-observed');
    return h.capture(...args);
  } }), error => error.message === '[REDACTED] [REDACTED]');
  assert(h.calls.some(c => c.args.includes('down')));
});
const request = () => ({ network: { cookie: { id: 7, owner_id: 42, domain: 'youtube.com', is_pool: false, version: 'a'.repeat(64) } } });
test('durable schema rejects missing reference, wrong owner and transient credentials', () => {
  assertYoutubeAuthRequest(request(), 7);
  for (const mutate of [r => delete r.network.cookie, r => r.network.cookie.owner_id = 99,
    r => r.network.cookie.version = 'bad', r => r.youtube_oauth = {}, r => r.params = { token: 'synthetic-native-access' }]) {
    const r = request(); mutate(r); assert.throws(() => assertYoutubeAuthRequest(r, 7));
  }
});
test('actual flattened BotApi media contract matches native journal and receipt', () => {
  const call = { method: 'sendvideo', status: 'success', chat_id: 42, cached: false, uploadLength: 1024 };
  const receipt = { status: 'succeeded', attempt: 1, planned: 1, delivered: 1, message_id: 9001,
    units: [{ attempt: 1, ordinal: 0, artifact_count: 1, status: 'sent', message_ids: '[9001]' }] };
  assertYoutubeAuthDelivery(call, receipt, 1);
  assert.throws(() => assertYoutubeAuthDelivery({ method: 'sendvideo', data: { chat_id: 42 } }, receipt, 1));
  assert.throws(() => assertYoutubeAuthDelivery({ ...call, cached: true }, receipt, 1));
  assert.throws(() => assertYoutubeAuthDelivery(call, { ...receipt, delivered: 2 }, 1));
  assert.throws(() => assertYoutubeAuthDelivery(call, receipt, 2));
});
test('scenario hook failure restores only changed settings and own cookie', async () => {
  const sql = [], uploads = [];
  const old = { youtube_auth_mode: 'cookies', proxy_enabled: 1, updated_at: 123 };
  await assert.rejects(runYoutubeAuthScenarios({ fixtureDir: '/unused', botApi: { calls: [], setExpectedUpload: x => uploads.push(x), enqueue() { throw Error('admission hook failed'); } },
    fixtureSql: async q => { sql.push(q); if (q.startsWith('SELECT json_object')) return JSON.stringify(old); if (q.startsWith('SELECT count')) return '0'; if (q.includes('RETURNING id')) return '7'; return ''; },
  }), /admission hook failed/);
  assert(sql.includes('DELETE FROM cookies WHERE id=7'));
  assert(sql.some(q => q.includes("youtube_auth_mode='cookies',proxy_enabled=1,updated_at=123")));
  assert(!sql.some(q => q.startsWith('DELETE FROM cookies WHERE user_id')));
  assert.equal(uploads.at(-1), null);
});
