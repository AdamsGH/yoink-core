import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
const [action = 'run', target = 'latest'] = process.argv.slice(2);
if (!['run', 'restore'].includes(action)) {
  console.error('Native backups are offline operations: just backup run, or just backup restore <archive.ybk>. Restore creates an inactive copy, never overwrites production.');
  process.exit(2);
}
const docker = (args, timeout = 30000) => execFileSync('docker', args, { encoding: 'utf8', timeout, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
let image;
const operatorImage = () => {
  image ??= docker(['image', 'inspect', 'yoink/rust-operations:native', '--format', '{{.Id}}']).trim();
  if (!/^sha256:[0-9a-f]{64}$/.test(image)) throw new Error('build the native operations image with just build backup');
  return image;
};
const stamp = new Date().toISOString().replaceAll(':', '-');
const root = resolve('.');
const operations = mounts => ['run', '--rm', '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp/yoink-work:rw,noexec,nosuid,size=64m,uid=10001,gid=10001,mode=700', ...mounts.flatMap(([source, destination, readonly]) => ['--mount', `type=bind,src=${root}/${source},dst=${destination}${readonly ? ',readonly' : ''}`]), operatorImage()];
let resume = false;
let containerId;
let failure;
try {
  if (action === 'restore') {
    let archive = target;
    if (archive === 'latest') archive = docker(['run', '--rm', '--pull', 'never', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--entrypoint', 'sh', '--mount', `type=bind,src=${root}/data/native-backups,dst=/archives,readonly`, operatorImage(), '-ec', 'ls -1t /archives/*.ybk | head -1']).trim().split('/').at(-1);
    if (!/^[a-zA-Z0-9][a-zA-Z0-9._-]*\.ybk$/.test(archive)) throw new Error('archive must be a basename within data/native-backups');
    docker([...operations([['data/native-backups', '/archives', true], ['data/native-restore', '/restore', false]]), 'restore', '--source', `/archives/${archive}`, '--destination', `/restore/restored-${stamp}.db`], 320000);
    console.log(`Verified inactive restore: data/native-restore/restored-${stamp}.db. No dispatcher or poller was started.`);
  } else {
    operatorImage();
    const container = JSON.parse(docker(['inspect', 'yoink']))[0];
    containerId = container.Id;
    if (container.Config.Entrypoint?.[0] !== '/usr/local/bin/yoink-server') throw new Error('yoink is not the native runtime; legacy backup requires explicit legacy tooling');
    if (!container.Config.Env?.includes('YOINK_RUST_DATABASE_PATH=/state/native.db') || !container.Mounts.some(mount => mount.Destination === '/state' && mount.Source === `${root}/data/native`)) throw new Error('running container does not own the configured native source');
    const worker = docker(['ps', '--filter', 'name=^/yoink-arq-worker$', '--format', '{{.ID}}']).trim();
    if (worker) throw new Error('legacy writer is still running; stop it before native maintenance');
    if (container.State.Running) {
      docker(['stop', '--time', '165', containerId], 180000);
      const stopped = JSON.parse(docker(['inspect', containerId]))[0];
      if (stopped.State.ExitCode !== 0 || stopped.State.OOMKilled) throw new Error('native shutdown did not complete normally; no backup attempted');
      resume = true;
    }
    docker([...operations([['data/native', '/state', false], ['data/native-backups', '/archives', false]]), 'backup', '--source', '/state/native.db', '--destination', `/archives/native-${stamp}.ybk`], 320000);
    console.log(`Verified native archive: data/native-backups/native-${stamp}.ybk. Archive is private plaintext; encrypt before external storage.`);
  }
} catch (error) {
  failure = error;
  console.error('Native backup/restore failed. Existing state and archives were not overwritten. Build the operator with just build backup; check private directory ownership, original shutdown and operator CLI receipts.');
} finally {
  if (resume) {
    try { docker(['start', containerId], 120000); }
    catch { console.error('Native restart failed; yoink remains stopped. Original container may have disappeared. Reconcile it before any explicit deployment and verify health.'); process.exitCode = 1; }
  }
}
if (failure) process.exitCode = 1;
