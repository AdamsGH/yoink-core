import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const image = process.argv[2];
assert.match(image ?? '', /^sha256:[a-f0-9]{64}$/, 'Supply the immutable content-worker image ID');
assert.equal(process.argv.length, 3);
const html = readFileSync(new URL('../worker/tests/content/article.html', import.meta.url), 'utf8');

function execute(task) {
  const name = `yoink-content-launcher-${randomUUID()}`;
  try {
    const request = { version: 2, operation_id: randomUUID(), attempt: 1, task, proxy_url: null };
    const child = spawnSync('docker', [
      'run', '--rm', '-i', '--name', name, '--pull', 'never', '--network', 'none',
      '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
      '--memory', '512m', '--cpus', '1', '--pids-limit', '128',
      '--tmpfs', '/tmp/yoink-work:size=16m,uid=10001,gid=10001,mode=0700,noexec,nosuid,nodev',
      '--entrypoint', '/usr/local/bin/yoink-worker-launcher', image,
    ], { input: `${JSON.stringify(request)}\n`, encoding: 'utf8', timeout: 30000, maxBuffer: 1024 * 1024 });
    assert.ifError(child.error);
    assert.equal(child.status, 0, child.stderr);
    const frame = JSON.parse(child.stdout);
    assert.equal(frame.version, request.version);
    assert.equal(frame.operation_id, request.operation_id);
    assert.equal(frame.attempt, request.attempt);
    return frame.event;
  } finally {
    const cleanup = spawnSync('docker', ['rm', '--force', name], { encoding: 'utf8', timeout: 10000 });
    assert.ifError(cleanup.error);
    assert.ok(cleanup.status === 0 || cleanup.stderr.includes('No such container'), 'Fixture container cleanup failed');
  }
}

assert.deepEqual(execute({ kind: 'content_get', url: 'http://127.0.0.1/secret', headers: {} }), { type: 'content_denied' });
for (const kind of ['content_readability', 'content_trafilatura']) {
  const task = { kind, html };
  if (kind === 'content_readability') task.url = 'https://example.com';
  const event = execute(task);
  assert.equal(event.type, 'content_completed');
  assert.ok(event.result.text.includes('Extract this real paragraph'));
  assert.ok(!event.result.text.includes('noise'));
}
console.log('Actual confined launcher: private destination denied; both pinned extractors passed.');
