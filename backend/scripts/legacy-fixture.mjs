// Disposable legacy PostgreSQL oracle. Never discovers deployment credentials or volumes.
import { execFile } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';

const exec = promisify(execFile);
const workspace = fileURLToPath(new URL('../', import.meta.url));
const migrations = fileURLToPath(new URL('../../src/db', import.meta.url));

export async function withLegacyFixture(body) {
  const name = `yoink-turso-import-${randomUUID()}`;
  const password = randomUUID();
  let created = false;
  let interrupted = false;
  async function command(binary, args, options = {}) {
    try {
      return await exec(binary, args, { cwd: workspace, timeout: 180_000, maxBuffer: 8 * 1024 * 1024, ...options });
    } catch (error) {
      const output = `${error.stdout ?? ''}${error.stderr ?? ''}`.replaceAll(password, '[fixture password]');
      throw new Error(`${binary} failed (${error.code ?? 'unknown'}): ${output}`);
    }
  }
  async function cleanup() {
    if (created) {
      await command('docker', ['rm', '--force', '--volumes', name]);
      created = false;
    }
  }
  const handlers = new Map(['SIGINT', 'SIGTERM'].map(signal => [signal, () => {
    interrupted = true;
    void cleanup().catch(error => console.error(error.message));
  }]));
  for (const [signal, handler] of handlers) process.once(signal, handler);
  try {
    await command('docker', ['run', '--detach', '--pull', 'never', '--name', name,
      '--label', 'yoink.fixture=turso-import', '--publish', '127.0.0.1::5432',
      '--tmpfs', '/var/lib/postgresql/data:rw', '--env', 'POSTGRES_USER=yoink_test',
      '--env', 'POSTGRES_DB=yoink_test', '--env', `POSTGRES_PASSWORD=${password}`, 'postgres:16-alpine']);
    created = true;
    let ready = false;
    for (let attempt = 0; attempt < 100 && !interrupted; attempt++) {
      try {
        await command('docker', ['exec', name, 'pg_isready', '-U', 'yoink_test', '-d', 'yoink_test']);
        ready = true;
        break;
      } catch { await sleep(100); }
    }
    if (!ready || interrupted) throw new Error('legacy fixture did not become ready');
    const migration = await command('docker', ['run', '--rm', '--pull', 'never',
      '--network', `container:${name}`, '--entrypoint', 'alembic',
      '--mount', `type=bind,source=${migrations},target=/fixture/db,readonly`,
      '--env', `database_url=postgresql+psycopg://yoink_test:${password}@127.0.0.1:5432/yoink_test`,
      'yoink/yoink:latest', '-c', '/fixture/db/alembic.ini', 'upgrade', 'head']);
    const revisionCount = [...(migration.stdout + migration.stderr).matchAll(/Running upgrade/g)].length;
    console.log(`Legacy fixture: ${revisionCount} actual Alembic revisions applied.`);
    const binding = await command('docker', ['port', name, '5432/tcp']);
    const port = binding.stdout.match(/^127\.0\.0\.1:(\d+)/m)?.[1];
    if (!port || interrupted) throw new Error('legacy fixture unavailable');
    const url = `postgresql://yoink_test:${password}@127.0.0.1:${port}/yoink_test`;
    const query = async sql => (await command('docker', ['exec', name, 'psql', '-X', '-v', 'ON_ERROR_STOP=1',
      '-U', 'yoink_test', '-d', 'yoink_test', '-qAtc', sql])).stdout.trim();
    return await body({ query, url, command, name });
  } finally {
    for (const [signal, handler] of handlers) process.removeListener(signal, handler);
    await cleanup();
    if (interrupted) throw new Error('legacy fixture interrupted');
  }
}
