import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { startHeldDns, resolverArgs, bridgeAddress, createOwnedDnsContainer } from './dns-harness.mjs';
import { createProcessRunner, createContainerLifecycle } from '../media-fixture/process-lifecycle.mjs';
import { createNativeDbClient } from '../media-fixture/native-db.mjs';
import { BotApiFixture } from '../media-fixture/bot-api.mjs';
import { TrustedServices, receiptProxy } from './services.mjs';
import { freePort, waitFor, api, initData, cleanup, report } from './helpers.mjs';
import { verifyNativeFailure } from './native-rejection.mjs';

export async function withRuntime(config, execute) {
  const actions = [], children = new Set(), containers = new Set();
  const id = randomBytes(8).toString('hex'), volumeName = `yoink-inbox-${id}`;
  const fixtureToken = randomBytes(24).toString('hex');
  const telegramToken = '123456:fixture_inbox_token';
  const secret = randomBytes(32).toString('hex');
  const { capture, docker } = createProcessRunner({ children });
  const { startContainer, helperContainer } = createContainerLifecycle({ docker, capture, containers, assert, runId: id });
  // Registered before any allocation; helpers are tracked even on nonzero exits.
  actions.push(async () => { for (const child of children) child.kill('SIGKILL'); });
  actions.push(async () => { const failures = []; for (const container of containers) { try { await docker(['rm', '--force', container]); } catch { failures.push(container); } } assert.equal(failures.length, 0, 'all owned containers removed'); });
  let appContainerId, aiTunnelContainerId, activeDns, interrupted = false, rejectedNumber = 0;
  async function expectNativeFailure(args, reason, input) {
    const name = `yoink-inbox-rejection-${id}-${rejectedNumber++}`;
    const container = await docker(['create', '--pull', 'never', '--name', name, ...(input === undefined ? [] : ['--interactive']), ...args]);
    assert.match(container, /^[0-9a-f]{12,64}$/i);
    containers.add(container);
    await capture('docker', ['start', '--attach', ...(input === undefined ? [] : ['--interactive']), container], { timeout: 30000, input }).catch(() => {});
    const state = JSON.parse(await docker(['inspect', '--format', '{{json .State}}', container]));
    const logs = await docker(['logs', container]);
    verifyNativeFailure(state, logs, reason);
  }
  const interrupt = () => { interrupted = true; if (appContainerId) void docker(['kill', '--signal', 'TERM', appContainerId]).catch(() => {}); };
  const checkInterrupted = () => { if (interrupted) throw new Error('fixture interrupted; owned resources will be cleaned'); };
  const fixturePort = await freePort(), httpPort = await freePort();
  const mount = ['--mount', `type=volume,source=${volumeName},target=/state`];
  const db = createNativeDbClient({ getAppContainerId: () => appContainerId, getFixturePort: () => fixturePort, fixtureToken, volumeName, fixtureImage: config['fixture-image'], helperContainer });
  async function stop({ liveWorker = false } = {}) {
    if (!appContainerId) return;
    const original = appContainerId;
    if (await docker(['inspect', '--format', '{{.State.Running}}', original]) === 'true') {
      await docker(['kill', '--signal', 'TERM', original]).catch(async error => {
        if (await docker(['inspect', '--format', '{{.State.Running}}', original]) === 'true') throw error;
      });
    }
    assert.equal(await docker(['wait', original], { timeout: 100000 }), '0', 'native admitted shutdown must drain normally');
    const state = JSON.parse(await docker(['inspect', '--format', '{{json .State}}', original]));
    assert.equal(state.OOMKilled, false);
    assert.equal(state.Running, false, 'original process is stopped');
    assert.equal(state.ExitCode, 0);
    assert.equal(state.Error, '');
    // fixture_main checks /proc children even with Inbox-only composition. Its
    // media workspace snapshot is not an Inbox workspace proof; inspect that below.
    const logs = await docker(['logs', original]);
    assert.match(logs, /fixture graceful shutdown verified: adapters drained, worker_children=0, workspaces=0/);
    if (liveWorker) {
      const snapshot = /fixture shutdown signal SIGTERM: worker_children=(\d+), workspaces=\d+/.exec(logs);
      assert(snapshot && Number(snapshot[1]) > 0, 'original SIGTERM snapshot must observe a live child before cancellation/reaping');
    }
    await docker(['rm', original]); containers.delete(original); appContainerId = undefined;
    if (aiTunnelContainerId) {
      await docker(['rm', '--force', aiTunnelContainerId]); containers.delete(aiTunnelContainerId); aiTunnelContainerId = undefined;
    }
  }
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, interrupt);
  try {
    const trusted = await new TrustedServices().start(); actions.push(() => trusted.close());
    const tunnelPort = await freePort();
    const tunnelSource = await readFile(new URL('../insight-fixture/ai-tunnel.py', import.meta.url), 'utf8');
    const bot = new BotApiFixture({ mediaBytes: Buffer.alloc(0) });
    const botPort = await bot.start(); actions.push(() => bot.close());
    const proxy = await receiptProxy(`http://127.0.0.1:${botPort}`); actions.push(() => proxy.close());
    // Confirm images exist without pulling/building. The server image embeds its
    // worker (the native launcher has fixed paths); never mount host Python.
    for (const image of new Set([config['server-image'], config['fixture-image'], config['worker-image'], config['operator-image']])) await docker(['image', 'inspect', image]);
    const fingerprints = [];
    for (const image of [config['server-image'], config['fixture-image'], config['worker-image']]) {
      fingerprints.push(await helperContainer(['--network', 'none', '--entrypoint', 'sh', image, '-c', 'sha256sum /opt/yoink-worker/main.py /usr/local/bin/yoink-worker-launcher']));
    }
    assert(fingerprints.every(value => value === fingerprints[0]), 'server embeds the supplied immutable worker/launcher; native Inbox does not launch a separate image');
    await docker(['volume', 'create', volumeName]);
    // Volume deletion must follow container deletion, even after failure.
    actions.unshift(() => docker(['volume', 'rm', volumeName]));
    await helperContainer(['--network', 'none', '--user', '0', ...mount, '--entrypoint', 'sh', config['server-image'], '-c', 'mkdir -p /state/private /state/inbox; chmod 0700 /state/private /state/inbox; chown 10001:10001 /state/private /state/inbox']);
    await helperContainer(['--network', 'none', ...mount, '--env', 'YOINK_RUST_DATABASE_PATH=/state/private/state.db', '--entrypoint', 'yoink-server', config['server-image'], 'migrate']);
    await db.batch("INSERT INTO users(id,role,language,theme,is_premium,created_at,updated_at) VALUES(42,'owner','en','macchiato',0,0,0),(43,'user','en','macchiato',0,0,0);");
    async function launch({ inbox = true, auth = true, telegram = true, groups = false, deadlines = false, dns = activeDns } = {}) {
      checkInterrupted(); assert(!appContainerId); assert(!(deadlines && dns), 'distinct deadline and live-worker budgets');
      const env = {
        YOINK_FIXTURE_BIND: `127.0.0.1:${fixturePort}`, YOINK_FIXTURE_TOKEN: fixtureToken,
        YOINK_RUST_DATABASE_PATH: '/state/private/state.db', YOINK_RUST_HTTP_BIND: `127.0.0.1:${httpPort}`,
        YOINK_RUST_INBOX_ENABLED: String(inbox), YOINK_RUST_AUTH_ENABLED: String(auth),
        YOINK_RUST_API_SECRET_KEY: secret, YOINK_RUST_TELEGRAM_TOKEN: telegramToken, YOINK_RUST_OWNER_ID: '42',
        YOINK_RUST_TELEGRAM_ENABLED: String(telegram), YOINK_RUST_TELEGRAM_GROUPS_ENABLED: String(groups), YOINK_RUST_BOT_API_URL: proxy.origin,
        YOINK_RUST_DOWNLOADS_ENABLED: 'false', YOINK_RUST_SHUTDOWN_TIMEOUT_SECS: '90',
        YOINK_RUST_INBOX_WORKSPACE: '/state/inbox', YOINK_RUST_INBOX_GITHUB_ENDPOINT: trusted.origin + '/',
        YOINK_RUST_INSIGHT_GATEWAY_BASE: trusted.origin + '/v1', YOINK_RUST_INSIGHT_GATEWAY_KEY: 'fixture-gateway', YOINK_RUST_INSIGHT_MODEL: 'fixture-model',
        YOINK_RUST_INSIGHT_AI_PROXY: `socks5://127.0.0.1:${tunnelPort}`,
        YOINK_RUST_INBOX_INTERVAL_MS: '100', YOINK_RUST_INBOX_MAX_ATTEMPTS: '2',
        ...(deadlines ? { YOINK_RUST_INBOX_JOB_TIMEOUT_SECS: '6', YOINK_RUST_INBOX_GITHUB_TIMEOUT_SECS: '4', YOINK_RUST_INBOX_EXECUTOR_TIMEOUT_SECS: '3', YOINK_RUST_INBOX_SOCKET_TIMEOUT_SECS: '2', YOINK_RUST_INBOX_COMPLETION_TIMEOUT_SECS: '3', YOINK_RUST_INBOX_DRAIN_TIMEOUT_SECS: '1' } : {}),
        ...(dns ? { YOINK_RUST_INBOX_EXECUTOR_TIMEOUT_SECS: '60' } : {}),
        TMPDIR: '/tmp', TURSO_TMPDIR: '/tmp',
      };
      appContainerId = await startContainer(['--name', `yoink-inbox-app-${id}`, '--network', 'host', '--workdir', '/state/inbox', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '128', '--memory', '512m', '--tmpfs', '/tmp:rw,nosuid,nodev,noexec,size=64m,mode=1777', ...mount, ...(dns ? resolverArgs(dns) : []),
        ...Object.entries(env).flatMap(([key, value]) => ['--env', `${key}=${value}`]), '--entrypoint', 'yoink-fixture-server', config['fixture-image']]);
      if (inbox) {
        const target = new URL(trusted.origin);
        aiTunnelContainerId = await startContainer(['--network', 'host', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '32', '--memory', '64m', '--entrypoint', '/usr/local/bin/python', config['fixture-image'], '-I', '-B', '-c', tunnelSource, String(tunnelPort), JSON.stringify({ [target.hostname]: [Number(target.port)] })]);
        await waitFor(async () => { try { return (await docker(['exec', appContainerId, '/usr/local/bin/python', '-I', '-B', '-c', 'import socket,sys; s=socket.create_connection(("127.0.0.1",int(sys.argv[1])),1); s.close(); print("ready")', String(tunnelPort)])) === 'ready'; } catch { return false; } }, 'owned Inbox AI tunnel');
      }
    }
    async function ready() { await waitFor(async () => { checkInterrupted(); try { return (await fetch(`http://127.0.0.1:${httpPort}/health/ready`, { signal: AbortSignal.timeout(1000) })).status === 200; } catch { return false; } }, 'actual native readiness'); }
    async function workspaceClean() { assert.equal(await helperContainer(['--network', 'none', ...mount, '--entrypoint', 'sh', config['server-image'], '-c', 'test -z "$(ls -A /state/inbox)" && echo clean']), 'clean'); }
    const request = api(`http://127.0.0.1:${httpPort}`);
    const login = async actor => (await request('/api/v1/auth/token', { method: 'POST', body: { init_data: initData(actor, telegramToken) } })).access_token;
    const startDns = async () => {
      const dns = await startHeldDns({ startContainer: args => createOwnedDnsContainer({ docker, containers }, args), docker, image: config['server-image'], runId: id, poll: (fn, label, timeout) => waitFor(() => { checkInterrupted(); return fn(); }, label, timeout), removeContainer: async container => { await docker(['rm', '--force', container]); containers.delete(container); } });
      activeDns = dns.address;
      const guarded = { ...dns, close: async () => { await dns.close(); if (activeDns === dns.address) activeDns = undefined; } };
      actions.push(() => guarded.close()); return guarded;
    };
    const observeDnsWorker = async address => {
      assert(appContainerId);
      const source = await readFile(new URL('./observe-worker.py', import.meta.url), 'utf8');
      const snapshot = JSON.parse(await docker(['exec', appContainerId, '/usr/local/bin/python', '-I', '-B', '-c', source, bridgeAddress(address)]));
      const servers = snapshot.resolver.split('\n').map(line => line.trim().split(/\s+/)).filter(parts => parts[0] === 'nameserver').map(parts => parts[1]);
      assert.deepEqual(servers, [address], 'only this container uses fake DNS; no host/public resolver fallback');
      assert.match(snapshot.resolver, /timeout:30/); assert.match(snapshot.resolver, /attempts:2/);
      return snapshot;
    };
    const ensureReady = async () => { if (!appContainerId) await launch(); await ready(); };
    const diagnostics = async () => {
      const logs = appContainerId ? await docker(['logs', appContainerId]).catch(() => 'native logs unavailable') : 'native process stopped';
      const jobs = await db.query('SELECT id,kind,state,attempts,last_error FROM inbox_jobs ORDER BY id DESC LIMIT 20').catch(() => 'job snapshot unavailable');
      const notifications = await db.query('SELECT sequence,state,attempts,error_code FROM inbox_notifications ORDER BY sequence DESC LIMIT 20').catch(() => 'notification snapshot unavailable');
      return report(`jobs:\n${jobs}\nnotifications:\n${notifications}\nnative logs:\n${logs}`, [secret, telegramToken, fixtureToken]);
    };
    await execute({ startDns, observeDnsWorker, ensureReady, diagnostics, config, db, bot, proxy, trusted, launch, ready, stop, request, login, waitFor: (fn, label, timeout) => waitFor(() => { checkInterrupted(); return fn(); }, label, timeout), workspaceClean, helperContainer, expectNativeFailure, mount, httpPort, docker, startContainer, secret, telegramToken });
    await stop(); await workspaceClean();
  } catch (error) {
    let logs = '';
    if (appContainerId) logs = await docker(['logs', appContainerId]).catch(() => 'native logs unavailable');
    throw new Error(report(`${error?.stack ?? error}\nBounded original-process logs:\n${logs}`, [secret, telegramToken, fixtureToken]));
  } finally {
    for (const signal of ['SIGINT', 'SIGTERM']) process.removeListener(signal, interrupt);
    // Stop app before dependent HTTP services/DB removal. Failure cleanup is not
    // counted as the graceful acceptance gate.
    if (appContainerId) { try { await docker(['rm', '--force', appContainerId]); containers.delete(appContainerId); } catch { /* tracked cleanup retries */ } }
    await cleanup(actions);
  }
}
