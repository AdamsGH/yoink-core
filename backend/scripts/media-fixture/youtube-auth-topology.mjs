import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';

export function validateYoutubeAuthTopology(config) {
  const { services, networks } = config;
  assert.equal(services?.worker?.network_mode, 'none', 'standalone worker must have no network');
  assert.equal(networks?.services?.internal, true, 'upstream network must be internal');
  assert(!networks.services.external, 'upstream network must be owned by the fixture');
  for (const name of ['gateway', 'youtubei', 'bgutil']) {
    const service = services?.[name];
    assert(service, `missing ${name}`);
    assert.deepEqual(Object.keys(service.networks ?? {}), ['services'], `${name} must be internal-only`);
    assert(!service.network_mode && !service.ports?.length, `${name} must not expose host networking/ports`);
  }
  assert.deepEqual(services.youtubei.command, ['node', 'test/synthetic-server.mjs']);
  assert(services.bgutil.command?.join(' ').includes('synthetic-pot'), 'bgutil must be synthetic');
  const mount = services.worker.volumes?.find(v => v.target === '/run/yoink-youtube-auth');
  assert(mount?.type === 'volume' && mount.source === 'auth-socket' && mount.read_only === true,
    'worker must mount only the fixed socket volume read-only');
  const hosts = services.worker.extra_hosts;
  if (Array.isArray(hosts)) assert.deepEqual(hosts, ['youtu.be=93.184.216.34']);
  else assert.deepEqual(hosts, { 'youtu.be': '93.184.216.34' });
}

export async function startSyntheticYoutubeAuth({ capture, runId }) {
  assert.equal(typeof capture, 'function');
  assert.match(String(runId), /^[a-zA-Z0-9_-]{1,80}$/);
  const project = `yoink-auth-${String(runId).toLowerCase()}-${randomUUID().slice(0, 8)}`;
  const compose = ['compose', '-p', project, '-f', 'docker-compose.youtube-auth.yaml', '-f', 'docker-compose.youtube-auth-fixture.yaml'];
  const docker = async (args, options = {}) => {
    let result;
    try { result = await capture('docker', args, { timeout: 30000, ...options }); }
    catch (error) {
      // capture combines stdout/stderr into rejection messages; never propagate token text.
      throw new Error(String(error.message ?? error).replaceAll(/synthetic-(?:native-)?(?:access|refresh)(?:-observed)?/g, '[REDACTED]'));
    }
    if (result.code !== undefined) assert.equal(result.code, 0, 'docker fixture command failed');
    assert(!/synthetic-(?:native-)?(?:access|refresh)/.test(result.output), 'synthetic credential exposed in topology output');
    return result.output.trim();
  };
  let closing;
  const close = () => closing ??= (async () => {
    await docker([...compose, 'down', '--volumes', '--remove-orphans', '--timeout', '150'], { timeout: 360000 });
    for (const kind of ['container', 'volume', 'network']) {
      const args = kind === 'container' ? ['ps', '-aq'] : [kind, 'ls', '-q'];
      assert.equal(await docker([...args, '--filter', `label=com.docker.compose.project=${project}`]), '', `residual own ${kind}s`);
    }
  })();
  try {
    const config = JSON.parse(await docker([...compose, 'config', '--format', 'json']));
    validateYoutubeAuthTopology(config);
    assert(!config.volumes?.['auth-socket']?.external, 'socket volume must be fixture-owned');
    if (config.volumes?.['auth-socket']?.name) assert.equal(config.volumes['auth-socket'].name, `${project}_auth-socket`);
    await docker([...compose, 'up', '-d', '--no-build', '--pull', 'never', 'gateway'], { timeout: 60000 });
    // Probe from the already-internal gateway; no helper bridge or host provider access.
    const probe = `const fs=require('fs'); const net=require('net'); (async()=>{if(!fs.statSync('/run/yoink-youtube-auth/adapter.sock').isSocket())throw Error('socket'); for(const u of ['http://youtubei:9173/health','http://bgutil:4416/ping']){const r=await fetch(u,{signal:AbortSignal.timeout(2000)});if(!r.ok)throw Error('health');}await new Promise((ok,no)=>{const s=net.connect('/run/yoink-youtube-auth/adapter.sock',()=>{s.end();ok()});s.on('error',no);s.setTimeout(2000,()=>s.destroy(Error('timeout')))});})().catch(()=>process.exit(1));`;
    let ready = false;
    for (let attempt = 0; attempt < 30; attempt++) {
      try { await docker([...compose, 'exec', '-T', 'gateway', 'node', '-e', probe], { timeout: 7000 }); ready = true; break; }
      catch { if (attempt < 29) await sleep(500); }
    }
    assert(ready, 'synthetic YouTube auth bounded readiness failed');
    return {
      dockerArgs: ['--mount', `type=volume,source=${project}_auth-socket,target=/run/yoink-youtube-auth,readonly`, '--add-host', 'youtu.be:93.184.216.34'],
      close,
    };
  } catch (error) {
    try { await close(); } catch (cleanup) { throw new AggregateError([error, cleanup], 'YouTube auth startup and cleanup failed'); }
    throw error;
  }
}
