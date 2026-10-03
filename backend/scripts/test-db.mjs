// Private, disposable native databases. Never reads .env or accepts a database path.
import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { chmod, mkdtemp, rename, rm, stat } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const args = process.argv.slice(2);
if (args.length > 1 || (args.length === 1 && !['--storage-only', '--profiles-only'].includes(args[0]))) {
  throw new Error('expected no arguments, --storage-only, or --profiles-only');
}
const storageOnly = args[0] === '--storage-only';
const profilesOnly = args[0] === '--profiles-only';
const focused = storageOnly || profilesOnly;
const exec = promisify(execFile);
const workspace = fileURLToPath(new URL('../', import.meta.url));
const binary = fileURLToPath(new URL('../target/debug/yoink-server', import.meta.url));
const directory = await mkdtemp(path.join(os.tmpdir(), 'yoink-native-db-'));
await chmod(directory, 0o700);
const marker = `private-database-${randomUUID()}`;
const databasePath = path.join(directory, `${marker}.db`);
const detachedPath = path.join(directory, 'detached.db');
const env = { PATH: process.env.PATH, YOINK_RUST_DATABASE_PATH: databasePath, YOINK_RUST_TELEGRAM_ENABLED: 'false' };
const children = new Set();
let interrupted = false;

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    interrupted = true;
    for (const child of children) child.kill('SIGTERM');
  });
}

async function command(program, arguments_, options = {}) {
  if (interrupted) throw new Error('fixture interrupted');
  try {
    const result = await exec(program, arguments_, {
      cwd: workspace, timeout: 180_000, maxBuffer: 8 * 1024 * 1024, ...options,
    });
    return result.stdout + result.stderr;
  } catch (error) {
    const output = `${error.stdout ?? ''}${error.stderr ?? ''}`.replaceAll(marker, '[private database]').slice(-24000);
    throw new Error(`${path.basename(program)} ${arguments_[0] ?? 'serve'} failed (exit ${error.code ?? 'unknown'}): ${output}`);
  }
}

async function exists(file) {
  try { await stat(file); return true; }
  catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}

async function checkSchema() {
  const output = await command(binary, ['check-schema'], { env });
  assert.match(output, /database schema ready/);
  assert.ok(!output.includes(marker), 'database paths must not leak into process diagnostics');
}

async function verifyProcess(shutdown) {
  const child = spawn(binary, [], { env: { ...env,
    YOINK_RUST_HTTP_BIND: '127.0.0.1:0',
    YOINK_RUST_DATABASE_HEALTH_INTERVAL_SECS: '1',
    YOINK_RUST_DATABASE_ACQUIRE_TIMEOUT_SECS: '1',
    YOINK_RUST_SHUTDOWN_TIMEOUT_SECS: '2',
  }, stdio: ['ignore', 'pipe', 'pipe'] });
  children.add(child);
  let output = '';
  let exited;
  let spawnError;
  let detached = false;
  child.on('error', error => { spawnError = error; });
  child.on('close', (code, signal) => { exited = { code, signal }; children.delete(child); });
  for (const stream of [child.stdout, child.stderr]) {
    stream.on('data', chunk => { output = (output + chunk).slice(-16384); });
  }
  async function until(predicate) {
    const deadline = Date.now() + 10000;
    while (!predicate()) {
      if (spawnError) throw spawnError;
      if (Date.now() > deadline) throw new Error(`database process deadline exceeded: ${output.replaceAll(marker, '[private database]')}`);
      await sleep(25);
    }
  }
  try {
    await until(() => exited || output.includes('diagnostic adapters ready'));
    assert.equal(exited, undefined, `server must start with migrated database: ${output.replaceAll(marker, '[private database]')}`);
    const address = output.match(/127\.0\.0\.1:\d+/)?.[0];
    assert.ok(address);
    const ready = await fetch(`http://${address}/health/ready`, { signal: AbortSignal.timeout(2000) });
    assert.equal(ready.status, 200);
    await assert.rejects(command(binary, ['check-schema'], { env, timeout: 15000 }), /database/i,
      'a second process must not open an owned database');
    assert.equal((await fetch(`http://${address}/health/ready`, { signal: AbortSignal.timeout(2000) })).status, 200);
    if (shutdown === 'dependency') {
      // An open descriptor does not prove that the durable pathname still exists.
      await rename(databasePath, detachedPath);
      detached = true;
      await until(() => exited);
      assert.equal(exited.code, 1);
      assert.match(output, /database health check failed/);
    } else {
      child.kill(shutdown === 'signal' ? 'SIGTERM' : 'SIGKILL');
      await until(() => exited);
      if (shutdown === 'signal') assert.equal(exited.code, 0, 'native database must close cleanly');
      else assert.equal(exited.signal, 'SIGKILL');
    }
    assert.ok(!output.includes(marker));
    console.log(`Native startup readiness, exclusive ownership and ${shutdown} shutdown passed.`);
  } finally {
    if (!exited && !spawnError) {
      child.kill('SIGKILL');
      await until(() => exited);
    }
    if (detached) {
      assert.equal(await exists(databasePath), false, 'failure handling must not recreate a missing database');
      await rename(detachedPath, databasePath);
    }
  }
  await checkSchema();
}

try {
  if (!focused) {
    for (const action of ['check-schema', 'serve']) {
      await assert.rejects(command(binary, [action], { env, timeout: 15000 }), /database|schema/i);
      assert.equal(await exists(databasePath), false, 'ordinary startup must not create a database');
    }
  }
  const suites = profilesOnly
    ? [['-p', 'yoink-downloads', '--test', 'profile', '--test', 'native_smoke']]
    : [['-p', 'yoink-db'], ['-p', 'yoink-storage'], ...(!storageOnly ? [['-p', 'yoink-identity'], ['-p', 'yoink-http'], ['-p', 'yoink-downloads']] : [])];
  for (const suite of suites) {
    const output = await command('cargo', ['test', '--locked', ...suite]);
    const counts = [...output.matchAll(/test result: ok\. (\d+) passed;/g)];
    const passed = counts.reduce((sum, match) => sum + Number(match[1]), 0);
    assert.ok(passed > 0, `${suite.join(' ')} must execute native contracts`);
    assert.doesNotMatch(output, /test result: ok\..* [1-9][0-9]* ignored;/, 'native contracts must not be skipped');
    console.log(`${suite.join(' ')}: ${passed} native contracts passed.`);
  }
  if (!focused) {
    for (const action of ['migrate', 'migrate']) {
      const output = await command(binary, [action], { env });
      assert.match(output, /database schema ready/);
      assert.ok(!output.includes(marker));
      console.log(`Native ${action} passed.`);
    }
    await checkSchema();
    for (const shutdown of ['signal', 'crash', 'dependency']) await verifyProcess(shutdown);
  }
} catch (error) {
  console.error(error.message.replaceAll(marker, '[private database]'));
  process.exitCode = 1;
} finally {
  if (children.size !== 0) {
    console.error('fixture cleanup could not confirm all child exits');
    process.exitCode = 1;
  } else {
    await rm(directory, { recursive: true, force: true });
  }
  if (interrupted) process.exitCode = 1;
}
