import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { settings, supervise } from '../supervisor.mjs';

async function fixture(t, modes) {
  const directory = mkdtempSync(join(tmpdir(), 'youtube-supervisor-'));
  const specs = Object.entries(modes).map(([name, mode]) => ({ name, args: [fileURLToPath(new URL('./child.mjs', import.meta.url)), directory, name, mode] }));
  const service = supervise(specs, { startup: 3000, shutdown: 200 }, async () => {
    assert.ok(Object.keys(modes).every(name => existsSync(join(directory, name))));
  });
  t.after(async () => { service.stop(1, false); await service.done; rmSync(directory, { recursive: true, force: true }); });
  await service.ready;
  return { directory, service };
}

test('shutdown drains the gateway before stopping providers', async t => {
  const { directory, service } = await fixture(t, { gateway: 'normal', youtubei: 'normal', bgutil: 'normal' });
  service.stop();
  assert.equal(await service.done, 0);
  const stopped = readFileSync(join(directory, 'stopped'), 'utf8').trim().split('\n');
  assert.equal(stopped[0], 'gateway');
  assert.deepEqual(stopped.slice(1).sort(), ['bgutil', 'youtubei']);
});

test('a provider crash terminates the entire bundle with failure', async t => {
  const { directory, service } = await fixture(t, { gateway: 'normal', youtubei: 'normal', bgutil: 'normal' });
  process.kill(Number(readFileSync(join(directory, 'bgutil'), 'utf8')), 'SIGKILL');
  assert.equal(await service.done, 1);
  assert.deepEqual(readFileSync(join(directory, 'stopped'), 'utf8').trim().split('\n').sort(), ['gateway', 'youtubei']);
});

test('shutdown kills a component that ignores SIGTERM', async t => {
  const { directory, service } = await fixture(t, { gateway: 'ignore', bgutil: 'ignore' });
  const pid = Number(readFileSync(join(directory, 'gateway'), 'utf8'));
  service.stop();
  assert.equal(await service.done, 0);
  assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
});

test('failed spawn exits instead of waiting forever for readiness', async () => {
  const service = supervise([{ name: 'gateway', command: '/nonexistent-youtube-fixture', args: [] }], { startup: 100, shutdown: 100 }, async () => { throw new Error('not ready'); });
  await assert.rejects(service.ready);
  assert.equal(await service.done, 1);
});

test('supervisor budgets reject unbounded values', () => {
  assert.throws(() => settings({ YOINK_YOUTUBE_SHUTDOWN_TIMEOUT_MS: '999999' }));
  assert.throws(() => settings({ YOINK_YOUTUBE_STARTUP_TIMEOUT_MS: 'NaN' }));
});
