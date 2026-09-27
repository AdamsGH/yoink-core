// Owns a disposable PostgreSQL fixture. Never reads .env or accepts a database URL.
import { execFile, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import assert from 'node:assert/strict';

const exec = promisify(execFile);
const workspace = fileURLToPath(new URL('../', import.meta.url));
const migrations = fileURLToPath(new URL('../../src/db', import.meta.url));
const storageOnly = process.argv[2] === '--storage-only';
const profilesOnly = process.argv[2] === '--profiles-only';
const focused = storageOnly || profilesOnly;
if (process.argv.length > (focused ? 3 : 2)) throw new Error('unsupported database fixture arguments');
const name = `yoink-rust-test-${randomUUID()}`;
const password = randomUUID();
let created = false;
let interrupted = false;
const binary = fileURLToPath(new URL('../target/debug/yoink-server', import.meta.url));

async function command(binary, args, options = {}) {
  try {
    const result = await exec(binary, args, { cwd: workspace, timeout: 180_000, maxBuffer: 8 * 1024 * 1024, ...options });
    return result.stdout + result.stderr;
  } catch (error) {
    const output = `${error.stdout ?? ''}${error.stderr ?? ''}`.replaceAll(password, '[fixture password]');
    throw new Error(`${binary} failed (exit ${error.code ?? 'unknown'}): ${output}`);
  }
}

async function cleanup() {
  if (created) {
    await command('docker', ['rm', '--force', '--volumes', name]);
    created = false;
  }
}

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    interrupted = true;
    // Cleanup in finally also handles a command failing because this container stopped.
    void cleanup().catch(error => console.error(error.message));
  });
}

try {
  // Local images only. No production volumes, Compose project or external network joins.
  await command('docker', ['run', '--detach', '--pull', 'never', '--name', name,
    '--label', 'yoink.fixture=rust-db', '--publish', '127.0.0.1::5432',
    '--tmpfs', '/var/lib/postgresql/data:rw',
    '--env', 'POSTGRES_USER=yoink_test', '--env', 'POSTGRES_DB=yoink_test',
    '--env', `POSTGRES_PASSWORD=${password}`, 'postgres:16-alpine']);
  created = true;
  let ready = false;
  for (let attempt = 0; attempt < 100 && !interrupted; attempt++) {
    try {
      await command('docker', ['exec', name, 'pg_isready', '-U', 'yoink_test', '-d', 'yoink_test']);
      ready = true;
      break;
    } catch {
      await sleep(100);
    }
  }
  if (!ready || interrupted) throw new Error('isolated PostgreSQL did not become ready');
  const binding = await command('docker', ['port', name, '5432/tcp']);
  const port = binding.match(/^127\.0\.0\.1:(\d+)/m)?.[1];
  if (!port) throw new Error('fixture has no loopback port binding');
  // Use the real Alembic chain, not a manually maintained copy of production tables.
  const migrationOutput = await command('docker', ['run', '--rm', '--pull', 'never',
    '--network', `container:${name}`, '--entrypoint', 'alembic',
    '--mount', `type=bind,source=${migrations},target=/fixture/db,readonly`,
    '--env', `database_url=postgresql+psycopg://yoink_test:${password}@127.0.0.1:5432/yoink_test`,
    'yoink/yoink:latest', '-c', '/fixture/db/alembic.ini', 'upgrade', 'head']);
  console.log(`Alembic: ${[...migrationOutput.matchAll(/Running upgrade/g)].length} revisions applied to fresh fixture.`);
  if (interrupted) throw new Error('fixture interrupted');
  const env = { ...process.env, YOINK_TEST_DATABASE_URL: `postgresql://yoink_test:${password}@127.0.0.1:${port}/yoink_test` };
  let rejected = focused;
  try {
    if (!focused) await command(binary, ['check-schema'], { env: { PATH: process.env.PATH, YOINK_RUST_DATABASE_URL: env.YOINK_TEST_DATABASE_URL } });
  } catch (error) {
    assert.match(error.message, /schema/i);
    assert.ok(!error.message.includes(password));
    rejected = true;
  }
  assert.ok(rejected, 'schema check must reject an unmigrated database');
  const absent = await command('docker', ['exec', name, 'psql', '-U', 'yoink_test', '-d', 'yoink_test', '-Atc', "SELECT to_regnamespace('yoink_runtime') IS NULL"]);
  assert.equal(absent.trim(), 't', 'schema check must not create schema');
  // The queue suite owns bootstrap from an absent runtime schema. Run it
  // explicitly before media cases, then legacy identity and process checks.
  // Serial execution also fences the deliberate schema-corruption fixtures.
  const suites = profilesOnly ? [['yoink-downloads', '']] : [
    ['yoink-storage', 'queue_persistence_contract'],
    ['yoink-storage', ''],
    ...(!storageOnly ? [['yoink-identity', ''], ['yoink-downloads', '']] : []),
  ];
  for (const [crate, filter] of suites) {
    const skip = crate === 'yoink-storage' && !filter ? ['--skip', 'queue_persistence_contract'] : [];
    const output = await command('cargo', ['test', '--locked', '-p', crate, filter, '--', '--ignored', '--test-threads=1', ...skip], { env });
    assert.match(output, /test result: ok\. [1-9][0-9]* passed;/, `${crate} must execute its database suite`);
    const passed = [...output.matchAll(/test result: ok\. (\d+) passed;/g)].reduce((sum, match) => sum + Number(match[1]), 0);
    console.log(`${crate}: ${passed} database contract suite passed.`);
  }
  if (!focused) await verifyProcess(env.YOINK_TEST_DATABASE_URL);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
} finally {
  try { await cleanup(); } catch (error) {
    console.error(`fixture cleanup failed for ${name}: ${error.message}`);
    process.exitCode = 1;
  }
  if (interrupted) process.exitCode = 1;
}

async function verifyProcess(databaseUrl) {
  const env = { PATH: process.env.PATH, YOINK_RUST_DATABASE_URL: databaseUrl };
  for (const commandName of ['migrate', 'migrate', 'check-schema']) {
    const output = await command(binary, [commandName], { env });
    assert.match(output, /database schema ready/);
    assert.ok(!output.includes(password));
  }
  for (const shutdown of ['signal', 'dependency']) {
    const child = spawn(binary, [], { env: { ...env,
      YOINK_RUST_HTTP_BIND: '127.0.0.1:0', YOINK_RUST_TELEGRAM_ENABLED: 'false',
      YOINK_RUST_DATABASE_HEALTH_INTERVAL_SECS: '1', YOINK_RUST_DATABASE_ACQUIRE_TIMEOUT_SECS: '1',
      YOINK_RUST_SHUTDOWN_TIMEOUT_SECS: '2',
    }, stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    let exited;
    let spawnError;
    child.on('error', error => { spawnError = error; });
    child.on('exit', (code, signal) => { exited = { code, signal }; });
    for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { output = (output + chunk).slice(-16384); });
    async function until(predicate) {
      const deadline = Date.now() + 8000;
      while (!predicate()) {
        if (spawnError) throw spawnError;
        if (Date.now() > deadline) throw new Error('database process test deadline exceeded');
        await sleep(25);
      }
    }
    try {
      await until(() => exited || output.includes('diagnostic adapters ready'));
      assert.equal(exited, undefined, 'server must start with migrated database');
      const address = output.match(/127\.0\.0\.1:\d+/)?.[0];
      assert.ok(address);
      const ready = await fetch(`http://${address}/health/ready`, { signal: AbortSignal.timeout(2000) });
      assert.equal(ready.status, 200);
      if (shutdown === 'signal') {
        child.kill('SIGTERM');
        await until(() => exited);
        assert.equal(exited.code, 0, 'database pool must drain cleanly');
      } else {
        await command('docker', ['stop', '--time', '2', name]);
        await until(() => exited);
        assert.equal(exited.code, 1);
        assert.match(output, /database health check failed/);
      }
      assert.ok(!output.includes(password));
      console.log(`Database startup readiness and ${shutdown} shutdown passed.`);
    } finally {
      if (!exited && !spawnError) {
        child.kill('SIGKILL');
        await until(() => exited);
      }
    }
  }
}
