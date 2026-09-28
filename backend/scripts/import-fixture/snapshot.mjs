import assert from 'node:assert/strict';
import { access, readdir, rm } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { start, assertNoSecrets } from './process.mjs';
import { nativeRequest } from './native.mjs';

const exists = file => access(file).then(() => true, () => false);

export async function snapshotChecks({ fixture, sourceUrl, disposable, importer, native, backend, secrets }) {
  const source = fixture.query;
  const initial = await source("SELECT id::text || E'\\t' || key_hash FROM api_keys ORDER BY id LIMIT 1");
  const [keyId, initialValue] = initial.split('\t');
  assert(/^\d+$/.test(keyId) && initialValue, 'synthetic API key is available');
  await source(`UPDATE api_keys SET key_hash='snapshot-value' WHERE id=${keyId}`);
  const target = await disposable();
  let lock;
  let child;
  try {
    lock = await blockSource(fixture);
    child = startImport(importer, backend, sourceUrl, target.db);
    await waitForActivity(source, child);
    // Update through the session holding ACCESS EXCLUSIVE. A separate writer
    // would block behind our own lock and could never release this handshake.
    lock.stdin.end(`UPDATE api_keys SET key_hash='new-value' WHERE id=${keyId}; COMMIT;\n`);
    assert.equal((await finish(lock, 10_000)).code, 0);
    const result = await finish(child, 60_000);
    assertNoSecrets(child.stdout + child.stderr, secrets);
    assert.equal(result.code, 0, child.stderr);
    const row = await nativeRequest(native, target.db, `SELECT key_hash FROM api_keys WHERE id=${keyId}`);
    assert.equal(row.rows[0][0], 'snapshot-value', 'snapshot excludes the update committed after snapshot establishment');
    assert.equal(await source(`SELECT key_hash FROM api_keys WHERE id=${keyId}`), 'new-value');
  } finally {
    if (child) await child.terminate();
    if (lock) await release(lock);
    await source(`UPDATE api_keys SET key_hash='${initialValue.replaceAll("'", "''")}' WHERE id=${keyId}`);
    await rm(target.root, { recursive: true, force: true });
  }

  const interrupted = await disposable();
  let blocker;
  let importerProcess;
  try {
    blocker = await blockSource(fixture);
    importerProcess = startImport(importer, backend, sourceUrl, interrupted.db);
    await waitForActivity(source, importerProcess);
    const result = await importerProcess.terminate(10_000);
    assert.equal(result.code, 1, 'SIGTERM is handled by importer cleanup, not a forced kill');
    assertNoSecrets(importerProcess.stdout + importerProcess.stderr, secrets);
    assert(importerProcess.stderr.includes('interrupted'));
    assert.equal(await exists(interrupted.db), false);
    assert.deepEqual(await readdir(interrupted.destinationDir), [], 'graceful interruption removes reservation and staging');
    // Release our blocker before querying its table.
    await release(blocker);
    blocker = undefined;
    assert.equal(await source(`SELECT key_hash FROM api_keys WHERE id=${keyId}`), initialValue);
  } finally {
    if (importerProcess) await importerProcess.terminate();
    if (blocker) await release(blocker);
    await rm(interrupted.root, { recursive: true, force: true });
  }
}

function startImport(importer, cwd, sourceUrl, destination) {
  return start(importer, ['--destination', destination, '--allow-loopback-plaintext'], {
    cwd, env: { PATH: process.env.PATH, HOME: process.env.HOME, YOINK_IMPORT_SOURCE_URL: sourceUrl },
  });
}

async function blockSource(fixture) {
  const proc = start('docker', ['exec', '-i', fixture.name, 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1',
    '-U', 'yoink_test', '-d', 'yoink_test'], { keepStdin: true });
  proc.stdin.write("BEGIN; LOCK TABLE public.api_keys IN ACCESS EXCLUSIVE MODE; SELECT 'IMPORT_FIXTURE_LOCK_READY';\n");
  const deadline = Date.now() + 10_000;
  try {
    while (!proc.stdout.includes('IMPORT_FIXTURE_LOCK_READY')) {
      if (proc.child.exitCode !== null || proc.spawnError) throw new Error('snapshot blocker exited before readiness');
      if (Date.now() >= deadline) throw new Error('snapshot blocker readiness deadline');
      await delay(20);
    }
    return proc;
  } catch (error) { await release(proc); throw error; }
}

async function release(proc) {
  if (!proc.stdin.destroyed && !proc.stdin.writableEnded) proc.stdin.end('ROLLBACK;\n');
  await finish(proc, 5000);
}
async function finish(proc, timeout) {
  let timer;
  try {
    const result = await Promise.race([proc.closed, new Promise(resolve => {
      timer = setTimeout(() => resolve(null), timeout);
    })]);
    if (result) return result;
    await proc.terminate();
    throw new Error('fixture process deadline exceeded');
  } finally { clearTimeout(timer); }
}
async function waitForActivity(query, child) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.child.exitCode !== null || child.spawnError) throw new Error('importer exited before snapshot lock wait');
    const active = await query("SELECT count(*) FROM pg_stat_activity WHERE application_name='yoink-native-import' AND backend_xmin IS NOT NULL AND wait_event_type='Lock'");
    if (Number(active) === 1) return;
    await delay(25);
  }
  throw new Error('importer did not establish a snapshot before waiting for the table lock');
}
