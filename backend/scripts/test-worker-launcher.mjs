import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';

// Match the deployment hardening and the executor's complete v2 request/settings.
// No external network, database, secrets or persistent container state.
const request = {
  version: 2, operation_id: '4f15ab1d-8f81-4886-999d-44c26db8b9ce', attempt: 1,
  source_url: 'http://127.0.0.1/blocked', task: { kind: 'download', media_mode: 'video' },
  options: { max_height: null, video_codec: 'h264', audio_codec: 'best', container: 'mp4', engine: 'auto', playlist: null, clips: [], gallery_zip: false },
  network: null,
};
const settings = {
  YOINK_WORKER_MAX_BYTES: '20971520', YOINK_WORKER_TOTAL_MAX_BYTES: '104857600',
  YOINK_WORKER_MAX_ITEMS: '100', YOINK_WORKER_SOCKET_TIMEOUT_SECS: '1',
  YOINK_WORKER_PROCESSING_TIMEOUT_SECS: '3',
  YOINK_WORKER_ALLOWED_PRIVATE_ORIGINS: '["http://127.0.0.1:9"]',
  YOINK_WORKER_ALLOWED_PROXY_ORIGINS: '[]',
};
const result = spawnSync('docker', [
  'run', '--rm', '--pull', 'never', '--network', 'none', '-i', '--read-only',
  '--tmpfs', '/tmp/yoink-work:rw,noexec,nosuid,size=256m,uid=10001,gid=10001,mode=0700',
  '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--pids-limit', '128', '--memory', '512m',
  ...Object.entries(settings).flatMap(([key, value]) => ['--env', `${key}=${value}`]),
  '--entrypoint', 'sh', 'yoink/rust-backend:experimental', '-c',
  'mkdir -m 700 /tmp/yoink-work/fixture && cd /tmp/yoink-work/fixture && exec /usr/local/bin/yoink-worker-launcher',
], { input: `${JSON.stringify(request)}\n`, encoding: 'utf8', timeout: 20000, maxBuffer: 65536 });
assert.ifError(result.error);
assert.equal(result.status, 0, `sandboxed worker failed: ${result.stderr}`);
assert.equal(result.stderr, '', 'sandboxed worker must not print raw diagnostics');
assert.deepEqual(JSON.parse(result.stdout), {
  version: request.version, operation_id: request.operation_id, attempt: request.attempt,
  event: { type: 'failure', class: 'source_unavailable' },
});
console.log('Default launcher: hardened container, isolated Python imports and full v2 request/settings passed.');
