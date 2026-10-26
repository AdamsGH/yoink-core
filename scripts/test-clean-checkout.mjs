import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, mkdtempSync, rmSync, symlinkSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const source = fileURLToPath(new URL('../', import.meta.url));
const directory = mkdtempSync(path.join(os.tmpdir(), 'yoink-checkout-'));
const checkout = path.join(directory, 'checkout');
const cleanup = () => rmSync(directory, { recursive: true, force: true });
process.once('SIGINT', () => { cleanup(); process.exit(130); });
process.once('SIGTERM', () => { cleanup(); process.exit(143); });
const env = { VITE_DEV_TOKEN: '', npm_config_cache: path.join(directory, 'npm-cache') };
for (const key of ['PATH', 'HOME', 'TMPDIR', 'LANG', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY', 'NODE_EXTRA_CA_CERTS']) {
  if (process.env[key] !== undefined) env[key] = process.env[key];
}
const run = (command, args, cwd = checkout) => {
  const result = spawnSync(command, args, { cwd, stdio: 'inherit', timeout: 600000, env });
  assert(!result.error && result.status === 0, `${command} failed: ${result.error?.message ?? result.status}`);
};
try {
  const state = spawnSync('git', ['status', '--porcelain'], { cwd: source, encoding: 'utf8', env, timeout: 30000 });
  assert(!state.error && state.status === 0 && state.stdout.trim() === '', 'Commit source changes before testing the committed checkout.');
  run('git', ['clone', '--quiet', '--no-hardlinks', source, checkout], directory);
  run('git', ['submodule', 'update', '--init', '--recursive']);
  copyFileSync(path.join(checkout, '.env.example'), path.join(checkout, '.env'));
  run('docker', ['compose', 'config', '--quiet']);
  run('just', ['--summary']);
  run('npm', ['ci', '--prefix', 'frontend']);
  for (const plugin of ['yoink-dl', 'yoink-stats', 'yoink-insight', 'yoink-inbox']) {
    symlinkSync('../../../frontend/node_modules', path.join(checkout, 'plugins', plugin, 'frontend/node_modules'));
  }
  run('just', ['verify-fe']);
  run('npm', ['run', 'build', '--prefix', 'frontend']);
  console.log('Fresh checkout: public submodule commits, Compose, frontend regressions, TypeScript and SPA build passed. No poller started.');
} finally {
  cleanup();
}
