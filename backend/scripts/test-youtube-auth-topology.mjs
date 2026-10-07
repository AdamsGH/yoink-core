import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { mkdirSync, writeFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
const root = fileURLToPath(new URL('../../', import.meta.url));
const project = `yoink-yt-auth-${randomUUID().slice(0, 12)}`;
const args = ['compose', '-p', project, '-f', 'backend/docker-compose.youtube-auth.yaml', '-f', 'backend/docker-compose.youtube-auth-fixture.yaml'];
function engine(command, input, action = command[0]) {
  const r = spawnSync('docker', command, { cwd: root, input, encoding: 'utf8', timeout: action === 'down' ? 360000 : 60000, maxBuffer: 1024 * 1024 });
  if (r.error || r.status !== 0) {
    const directory = `${root}backend/media-reports/${project}`;
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    const report = `${directory}/${action}-${randomUUID()}.log`;
    writeFileSync(report, `status=${r.status}\nerror=${r.error?.message ?? ''}\n${r.stdout ?? ''}\n${r.stderr ?? ''}`, { mode: 0o600 });
    throw new Error(`docker ${action} failed, status ${r.status}. Restricted diagnostics: ${report}`);
  }
  return r;
}
function docker(command, input) {
  return engine([...args, ...command], input, command[0]);
}
const topology = JSON.parse(docker(['config', '--format', 'json']).stdout);
assert.equal(topology.services.worker.network_mode, 'none');
assert.equal(topology.services.worker.user, '10001:10001');
assert.equal(topology.services.worker.volumes.length, 1);
assert.equal(topology.services.worker.volumes[0].target, '/run/yoink-youtube-auth');
assert.equal(topology.services.worker.volumes[0].read_only, true);
assert.equal(topology.services.worker.tmpfs.length, 1);
assert.ok(topology.services.worker.tmpfs[0].startsWith('/tmp/yoink-work:'));
assert.equal(topology.networks.services.internal, true);
for (const name of ['worker', 'gateway', 'youtubei', 'bgutil']) {
  // Compose JSON normalizes 150s to "2m30s". Never truncate media drain.
  const duration = topology.services[name].stop_grace_period;
  const parts = [...String(duration).matchAll(/(\d+(?:\.\d+)?)(h|m|s)/g)];
  assert.equal(parts.map(part => part[0]).join(''), String(duration));
  const seconds = parts.reduce((sum, part) => sum + Number(part[1]) * ({ h: 3600, m: 60, s: 1 }[part[2]]), 0);
  assert.ok(seconds >= 150);
}
for (const name of ['youtubei', 'bgutil', 'gateway']) {
  assert.deepEqual(Object.keys(topology.services[name].networks), ['services']);
  assert.equal(topology.services[name].read_only, true);
  assert.equal(topology.services[name].init, true);
  assert.ok(topology.services[name].mem_limit > 0 && topology.services[name].pids_limit > 0 && topology.services[name].cpus > 0);
  assert.ok(!topology.services[name].ports?.length);
  for (const mount of topology.services[name].tmpfs ?? []) assert.ok(mount.startsWith('/'), 'tmpfs options must stay inside one quoted mount string');
}
if (process.argv.includes('--static')) {
  console.log('Static fixture topology: internal-only services, networkless UID10001 worker, socket-only RO mount, resource ceilings passed.');
  process.exit(0);
}
const request = {
  version: 2, operation_id: '4f15ab1d-8f81-4886-999d-44c26db8b9ce', attempt: 1,
  source_url: 'https://youtu.be/abcdefghijk', task: { kind: 'download', media_mode: 'video' },
  options: { clips: [{ start_seconds: 0, end_seconds: 0.5 }] },
  network: { youtube_oauth: { access_token: 'synthetic-access', refresh_token: 'synthetic-refresh', expires_at: '2099-01-01T00:00:00Z' } },
};
try {
  // No build and no pulling implicit dependencies; coordinator supplies reviewed images.
  docker(['up', '-d', '--no-build', '--pull', 'never', 'gateway']);
  for (let n = 0; ; n++) {
    const r = spawnSync('docker', [...args, 'exec', '-T', 'gateway', 'node', '-e', "fetch('http://youtubei:9173/health').then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))"], { cwd: root, encoding: 'utf8' });
    if (r.status === 0) break;
    assert.ok(n < 100, 'synthetic upstream not ready'); await new Promise(r => setTimeout(r, 100));
  }
  const tooling = docker(['run', '--rm', '--no-deps', '-T', '--entrypoint', 'node', 'youtubei', '-e', "console.log(process.version);console.log(require('child_process').execFileSync('ffmpeg',['-version'],{encoding:'utf8'}).split('\\n')[0])"]);
  console.log(tooling.stdout.trim());
  const suite = docker(['run', '--rm', '--no-deps', '-T', '-e', 'YOINK_CONTAINER_FFMPEG_TESTS=1', '--entrypoint', 'npm', 'youtubei', 'test']);
  console.log(suite.stdout.split('\n').filter(line => /^# (tests|pass|fail|skipped)\b/.test(line)).join('\n'));
  docker(['exec', '-T', 'gateway', 'node', '-e', "const h=require('http');Promise.all(['/pot/ping','/pot'].map(path=>new Promise((ok,bad)=>{const q=h.request({socketPath:'/run/yoink-youtube-auth/adapter.sock',method:'POST',path},r=>{let s='';r.on('data',c=>s+=c);r.on('end',()=>{const b=JSON.parse(s);r.statusCode===200&&(b.version==='2.0.0'||b.poToken==='synthetic-pot')?ok():bad()})});q.on('error',bad);q.end('{}')}))).catch(()=>process.exit(1))"]);
  const launch = (payload, extra = []) => docker(['run', '--rm', '--no-deps', '-T', ...extra, 'worker'], JSON.stringify(payload) + '\n');
  const normal = launch(request);
  assert.equal(normal.stderr.includes('synthetic-access'), false);
  const frames = normal.stdout.trim().split('\n').map(JSON.parse);
  const complete = frames.find(f => f.event.type === 'completed');
  assert.ok(complete, 'native confined worker must deliver processed artifact frame');
  assert.equal(complete.event.artifacts.length, 1);
  assert.ok(complete.event.artifacts[0].size_bytes > 0);
  assert.ok(!JSON.stringify(frames).includes('/tmp/youtubei') && !JSON.stringify(frames).includes('synthetic-access'));
  const byte = launch(request, ['-e', 'YOINK_WORKER_MAX_BYTES=1', '-e', 'YOINK_WORKER_TOTAL_MAX_BYTES=1']);
  assert.ok(byte.stdout.includes('resource_limit'));
  const refresh = launch({ ...request, network: { youtube_oauth: { ...request.network.youtube_oauth, access_token: 'synthetic-refresh-observed' } } });
  assert.ok(refresh.stdout.includes('source_unavailable'));
  const logs = docker(['logs', '--no-color']).stdout;
  for (const secret of ['synthetic-access', 'synthetic-refresh', '/tmp/youtubei/']) assert.ok(!logs.includes(secret));
  console.log('Synthetic fixed-socket topology, native launcher, real ffmpeg, resource/refresh rejection passed. No provider acceptance claimed.');
} finally {
  for (let attempt = 0; ; attempt++) {
    docker(['down', '-v', '--remove-orphans']);
    await new Promise(resolve => setTimeout(resolve, 1000));
    const filter = `label=com.docker.compose.project=${project}`;
    const containers = engine(['ps', '-a', '-q', '--filter', filter]).stdout.trim();
    const volumes = engine(['volume', 'ls', '-q', '--filter', filter]).stdout.trim();
    const networks = engine(['network', 'ls', '-q', '--filter', filter]).stdout.trim();
    if (!containers && !volumes && !networks) break;
    assert.ok(attempt < 2, `Fixture resources remain for project ${project}`);
  }
}
