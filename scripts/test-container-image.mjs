import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const [component, image, version] = process.argv.slice(2);
assert(['backend', 'operations', 'importer'].includes(component), 'Expected a Rust release image.');
assert(image && !image.startsWith('-'), 'Expected an image reference.');
const run = (args, optional = false) => {
  const result = spawnSync('docker', args, { encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024 });
  if (result.error || result.status !== 0) {
    if (optional) return null;
    throw new Error(`Container smoke command ${args[0]} failed: ${result.error?.message ?? result.stderr}`);
  }
  return result.stdout.trim();
};
const isolation = ['--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--memory', '512m', '--pids-limit', '64', '--tmpfs', '/tmp/yoink-work:rw,noexec,nosuid,nodev,size=64m,uid=10001,gid=10001,mode=0700'];
if (component !== 'backend') {
  const output = run(['run', '--rm', '--pull', 'never', ...isolation, image, '--help']);
  assert(output.includes(component === 'operations' ? 'yoink-ops' : 'yoink-import'));
  console.log(`${component}: isolated operator CLI passed.`);
} else {
  const name = `yoink-image-smoke-${randomUUID()}`;
  const cleanup = () => {
    const result = spawnSync('docker', ['rm', '-f', name], { encoding: 'utf8', timeout: 30000 });
    if (result.error || result.status !== 0 && !result.stderr.includes('No such container')) throw new Error(`Cannot remove owned smoke container ${name}.`);
  };
  process.once('SIGINT', () => { cleanup(); process.exit(130); });
  process.once('SIGTERM', () => { cleanup(); process.exit(143); });
  try {
    run(['run', '-d', '--pull', 'never', '--name', name, ...isolation,
      '-e', 'YOINK_RUST_HTTP_BIND=127.0.0.1:8088', '-e', 'YOINK_RUST_TELEGRAM_ENABLED=false',
      '-e', 'YOINK_RUST_STARTUP_TIMEOUT_SECS=5', '-e', 'YOINK_RUST_SHUTDOWN_TIMEOUT_SECS=2', image]);
    const probe = 'import json,urllib.request; print(json.dumps([json.load(urllib.request.urlopen("http://127.0.0.1:8088"+p, timeout=2)) for p in ["/health/ready","/api/v1/system"]]))';
    const deadline = Date.now() + 30000;
    let snapshot;
    while (Date.now() < deadline) {
      assert.equal(run(['inspect', name, '--format', '{{.State.Running}}']), 'true', 'runtime exited before readiness');
      const output = run(['exec', name, 'python', '-c', probe], true);
      if (output !== null) { snapshot = JSON.parse(output); break; }
      await delay(200);
    }
    assert(snapshot, 'runtime readiness deadline exceeded');
    assert.equal(snapshot[1].ready, true);
    assert.equal(snapshot[1].version, version);
    run(['stop', '--time', '10', name]);
    assert.equal(run(['inspect', name, '--format', '{{.State.ExitCode}}']), '0', 'SIGTERM must finish cleanly');
    console.log('Runtime: isolated readiness, release version and SIGTERM passed. No database or Telegram polling enabled.');
  } finally {
    cleanup();
  }
}
