import assert from 'node:assert/strict';

export async function storageContracts(c) {
  const token = await c.login(42);
  const heldBefore = c.trusted.held;
  c.trusted.holdRepository = true;
  let item;
  try {
    item = await c.request('/api/v1/inbox/items', { token, method: 'POST', body: { url: 'https://github.com/fixture/storage' }, expected: 201 });
    await c.waitFor(() => c.trusted.held > heldBefore, 'actual storage snapshot job admitted');
    await c.stop();
  } finally { c.trusted.holdRepository = false; }
  assert.equal(await c.db.query(`SELECT state,attempts FROM inbox_jobs WHERE item_id=${item.id} AND kind='enrich'`), 'ready|0');
  const before = await c.db.query('SELECT count(*) FROM runtime_migrations');
  assert(Number(before) >= 23, 'native schema includes durable text migration');
  await c.helperContainer(['--network', 'none', ...c.mount, '--env', 'YOINK_RUST_DATABASE_PATH=/state/private/state.db', '--entrypoint', 'yoink-server', c.config['server-image'], 'migrate']);
  assert.equal(await c.db.query('SELECT count(*) FROM runtime_migrations'), before, 'migration rerun idempotent');
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'python', c.config['server-image'], '-c', "import sqlite3; from pathlib import Path; p='/state/private/unmigrated.db'; src=sqlite3.connect(':memory:'); dst=sqlite3.connect(p); src.backup(dst); dst.close(); src.close(); assert Path(p).read_bytes()[:16] == b'SQLite format 3\\x00'"]);
  const unmigratedToken = 'fixture-unmigrated-token-1234567890123456';
  const emptyDb = ['--network', 'none', ...c.mount, '--env', 'YOINK_RUST_DATABASE_PATH=/state/private/unmigrated.db', '--env', `YOINK_FIXTURE_TOKEN=${unmigratedToken}`, '--entrypoint', 'yoink-fixture-server', c.config['fixture-image']];
  await c.expectNativeFailure([...emptyDb, 'sql'], /^database schema check failed\s*$/, JSON.stringify({ token: unmigratedToken, sql: 'SELECT 1', mode: 'query' }) + '\n');
  await c.expectNativeFailure(['--env', 'YOINK_FIXTURE_BIND=127.0.0.1:29173', ...emptyDb, 'serve'], /^database schema check failed\s*$/);
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'sh', c.config['server-image'], '-c', 'printf invalid > /state/private/invalid.ybk; chmod 600 /state/private/invalid.ybk']);
  const opsHelp = await c.helperContainer(['--network', 'none', '--entrypoint', 'yoink-ops', c.config['operator-image'], '--help']);
  assert(opsHelp.includes('--source') && opsHelp.includes('--destination'), 'operator image supplies actual yoink-ops');
  await c.expectNativeFailure(['--network', 'none', ...c.mount, '--entrypoint', 'yoink-ops', c.config['operator-image'], 'restore', '--source', '/state/private/invalid.ybk', '--destination', '/state/private/restored.db'], /^backup is truncated, corrupt, unsupported, or exceeds the byte limit\s*$/);
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'sh', c.config['server-image'], '-c', 'test ! -e /state/private/restored.db && test ! -e /state/private/restored.db.yoink-lock']);
  assert.equal(await c.db.query('SELECT count(*) FROM runtime_migrations'), before, 'failure leaves owning migrated DB unchanged');
  const jobs = await c.db.query('SELECT id,state,attempts FROM inbox_jobs ORDER BY id');
  assert(jobs.length > 0, 'backup preserves actual admitted Inbox job state');
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'yoink-ops', c.config['operator-image'], 'backup', '--source', '/state/private/state.db', '--destination', '/state/private/snapshot.ybk']);
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'yoink-ops', c.config['operator-image'], 'restore', '--source', '/state/private/snapshot.ybk', '--destination', '/state/private/restored-good.db']);
  const restored = JSON.parse(await c.helperContainer(['--network', 'none', ...c.mount, '--env', 'YOINK_RUST_DATABASE_PATH=/state/private/restored-good.db', '--env', 'YOINK_FIXTURE_TOKEN=fixture-restored-token-1234567890123456', '--entrypoint', 'yoink-fixture-server', c.config['fixture-image'], 'sql'], 30000, JSON.stringify({ token: 'fixture-restored-token-1234567890123456', sql: 'SELECT id,state,attempts FROM inbox_jobs ORDER BY id', mode: 'query' }) + '\n'));
  assert.equal(restored.ok, true);
  assert.equal(restored.rows.map(row => row.join('|')).join('\n'), jobs, 'actual backup/restore preserves durable Inbox job states');
  await c.launch(); await c.ready();
  await c.waitFor(async () => (await c.request(`/api/v1/inbox/items/${item.id}`, { token })).status === 'classified', 'snapshotted actual job resumes');
}

export async function importContracts(c) {
  if (!c.config['importer-image']) throw new Error('DEPENDENCY: --importer-image required for actual import failure gate; backend/Dockerfile operations target contains yoink-ops only');
  const importerHelp = await c.helperContainer(['--network', 'none', '--entrypoint', 'yoink-import', c.config['importer-image'], '--help']);
  assert(importerHelp.includes('--destination'), 'importer image supplies actual yoink-import');
  await c.expectNativeFailure(['--network', 'none', ...c.mount, '--env', 'YOINK_IMPORT_SOURCE_URL=not-a-postgres-url', '--entrypoint', 'yoink-import', c.config['importer-image'], '--destination', '/state/private/import-failed.db'], /^import failed: invalid importer configuration\s*$/);
  await c.helperContainer(['--network', 'none', ...c.mount, '--entrypoint', 'sh', c.config['server-image'], '-c', 'test ! -e /state/private/import-failed.db && test ! -e /state/private/import-failed.db.yoink-lock']);
}
