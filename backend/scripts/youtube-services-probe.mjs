import assert from 'node:assert/strict';
import { statSync, readdirSync, readFileSync } from 'node:fs';
import http from 'node:http';
import { healthy } from '/opt/youtube-bundle/health.mjs';

assert.equal(process.getuid(), 10001);
await healthy();
await new Promise((resolve, reject) => {
  const request = http.request({ socketPath: process.env.YOINK_YOUTUBE_AUTH_SOCKET, path: '/pot/ping', method: 'POST', agent: false }, response => {
    response.resume();
    response.on('end', () => response.statusCode === 200 ? resolve() : reject(new Error('gateway provider route unavailable')));
    response.on('error', reject);
  });
  request.setTimeout(2000, () => request.destroy(new Error('provider route timeout')));
  request.on('error', reject);
  request.end('{}');
});
const directory = statSync('/run/yoink-youtube-auth');
assert.equal(directory.uid, 10001);
assert.equal(directory.mode & 0o777, 0o700);
assert.equal(statSync(process.env.YOINK_YOUTUBE_AUTH_SOCKET).mode & 0o777, 0o600);
if (process.argv.includes('--crash-provider')) {
  const provider = readdirSync('/proc').filter(name => /^\d+$/.test(name)).find(name => {
    try { return readFileSync(`/proc/${name}/cmdline`, 'utf8').split('\0').includes('build/main.js'); } catch { return false; }
  });
  assert(provider, 'real bgutil process must be running');
  process.kill(Number(provider), 'SIGTERM');
} else console.log('Bundle readiness and fresh-volume ownership passed as UID10001.');
