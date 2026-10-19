import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { youtubeImage } from './release.mjs';

const name = `yoink-youtube-check-${randomUUID().slice(0, 12)}`;
const volume = `${name}-socket`;
const image = youtubeImage(fileURLToPath(new URL('../../', import.meta.url)));
function docker(args, optional = false) {
  const result = spawnSync('docker', args, { encoding: 'utf8', timeout: 30000, maxBuffer: 1048576 });
  if (result.error || result.status !== 0) {
    if (optional) return null;
    throw new Error(`docker ${args[0]} failed: ${result.error?.message ?? result.stderr}`);
  }
  return result.stdout.trim();
}
try {
  docker(['run', '-d', '--pull', 'never', '--name', name, '--init', '--network', 'none', '--user', '10001:10001', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--memory', '1536m', '--cpus', '2', '--pids-limit', '192', '--tmpfs', '/tmp:size=512m,uid=10001,gid=10001,mode=0700,noexec,nosuid,nodev', '--mount', `type=volume,source=${volume},target=/run/yoink-youtube-auth`, '--mount', `type=bind,source=${fileURLToPath(new URL('./youtube-services-probe.mjs', import.meta.url))},target=/verify/probe.mjs,readonly`, '--mount', `type=bind,source=${fileURLToPath(new URL('../../docker/youtubei-service/test', import.meta.url))},target=/opt/youtubei/test,readonly`, '-e', 'HTTP_PROXY=', '-e', 'HTTPS_PROXY=', '-e', 'ALL_PROXY=', image]);
  const deadline = Date.now() + 60000;
  while (docker(['inspect', name, '--format', '{{.State.Health.Status}}']) !== 'healthy') {
    assert.equal(docker(['inspect', name, '--format', '{{.State.Running}}']), 'true', 'all bundled components must survive startup');
    assert(Date.now() < deadline, 'real bundled components must become healthy');
    await delay(200);
  }
  console.log(docker(['exec', name, 'node', '/verify/probe.mjs']));
  console.log(docker(['exec', '-e', 'YOINK_CONTAINER_FFMPEG_TESTS=1', name, 'node', '--test', '/opt/youtubei/test/service.test.mjs', '/opt/youtubei/test/ffmpeg.test.mjs']));
  docker(['exec', name, 'node', '/verify/probe.mjs', '--crash-provider']);
  assert.equal(docker(['wait', name]), '1', 'a provider crash must stop the entire container');
  console.log('Real bgutil failure stopped the bundle with exit code 1.');
} finally {
  const removed = docker(['rm', '-f', name], true);
  const cleaned = docker(['volume', 'rm', volume], true);
  if (removed === null || cleaned === null) {
    console.error(`Verify cleanup of owned fixture ${name} and volume ${volume}.`);
    process.exitCode = 1;
  }
}
