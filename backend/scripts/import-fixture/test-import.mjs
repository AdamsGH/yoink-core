#!/usr/bin/env node
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile, chmod, lstat, symlink, stat, access } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { withLegacyFixture } from '../legacy-fixture.mjs';
import { run, assertNoSecrets } from './process.mjs';
import { snapshotChecks } from './snapshot.mjs';
import { tlsChecks } from './tls.mjs';
import { compareAllContent } from './content.mjs';
import { edgeChecks } from './edges.mjs';
import { nativeRequest } from './native.mjs';
import { seedSynthetic } from './synthetic.mjs';
import { operationsChecks } from './operations.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const backend = path.resolve(here, '../..');
const importer = path.join(backend, 'target/debug/yoink-import');
const native = path.join(backend, 'target/debug/yoink-fixture-server');
const contract = JSON.parse(await readFile(path.join(backend, 'schema/legacy-postgres.json'), 'utf8'));
const expectedTables = contract.tables.map(table => table.name).filter(name => name !== 'alembic_version');
const passwordMarker = 'IMPORT_FIXTURE_FAKE_PASSWORD_7f3a';
const rowMarker = 'IMPORT_FIXTURE_ROW_MARKER_9c21';
const image = process.argv[2] || undefined;
assert(process.argv.length <= 3, 'expected at most one optional importer image');
const secrets = [passwordMarker, rowMarker];

async function disposable() {
  const root = await mkdtemp(path.join(os.tmpdir(), 'yoink-import-fixture-'));
  await chmod(root, 0o700);
  const sourceDir = path.join(root, 'source');
  const destinationDir = path.join(root, 'destination');
  await mkdir(sourceDir, { mode: 0o700 });
  await mkdir(destinationDir, { mode: 0o700 });
  return { root, sourceDir, destinationDir, db: path.join(destinationDir, 'imported.db') };
}
const exists = async p => access(p).then(() => true, () => false);
const quote = s => "'" + String(s).replaceAll("'", "''") + "'";

await withLegacyFixture(async fixture => {
  assert.equal(expectedTables.length, 41, 'legacy catalog must have all 41 application tables');
  const query = fixture.query;
  const sourceParsed = new URL(fixture.url);
  sourceParsed.username = 'yoink_import';
  sourceParsed.password = passwordMarker;
  const sourceUrl = sourceParsed.toString();
  await query(`CREATE ROLE yoink_import LOGIN PASSWORD '${passwordMarker}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
    GRANT CONNECT ON DATABASE yoink_test TO yoink_import;
    GRANT USAGE ON SCHEMA public TO yoink_import;
    GRANT SELECT ON ALL TABLES IN SCHEMA public TO yoink_import;
    GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO yoink_import`);

  const runImport = (url, db, { loopback = false, timeout = 120_000, env = {}, flags = [] } = {}) => run(
    importer, ['--destination', db, ...(loopback ? ['--allow-loopback-plaintext'] : []), ...flags],
    { cwd: backend, env: { PATH: process.env.PATH, HOME: process.env.HOME, YOINK_IMPORT_SOURCE_URL: url, ...env }, timeout },
  );
  const failure = async (action, label) => {
    let error;
    try { await action(); } catch (caught) { error = caught; }
    assert(error instanceof Error, `${label}: expected command failure`);
    assertNoSecrets(`${error.stdout ?? ''}${error.stderr ?? ''}${error.message}`, secrets);
    assert.equal(error.category, 'exit', `${label}: must fail explicitly, not time out or fail to launch`);
    return error;
  };
  const missingTarget = async db => assert.equal(await exists(db), false, 'failed import must not leave final target');
  const successfulPrerequisite = await disposable();
  try {
    const countsBefore = await sourceCounts(query, expectedTables);
    const digestsBefore = await sourceDigests(query, expectedTables);
    const synthetic = await seedSynthetic(query, contract);
    const seededCounts = await sourceCounts(query, expectedTables);
    assert(expectedTables.every(name => seededCounts[name] >= 1), 'synthetic source contains a record for every application table');
    const beforeDigest = await sourceDigests(query, expectedTables);
    const result = await runImport(sourceUrl, successfulPrerequisite.db, { loopback: true });
    assertNoSecrets(result.stdout + result.stderr, secrets);
    const file = await stat(successfulPrerequisite.db);
    assert(file.isFile() && (file.mode & 0o077) === 0, 'native database permissions are private');
    const nativeNames = await nativeRequest(native, successfulPrerequisite.db,
      'SELECT name FROM sqlite_schema WHERE type=\'table\'');
    assert(nativeNames.ok);
    const names = new Set(nativeNames.rows.map(row => row[0]));
    assert.deepEqual(expectedTables.filter(name => !names.has(name)), [], 'native database includes every application table');
    for (const table of expectedTables) {
      const row = await nativeRequest(native, successfulPrerequisite.db,
        `SELECT CAST(COUNT(*) AS TEXT) FROM "${table.replaceAll('"', '""')}"`);
      assert.equal(Number(row.rows[0][0]), seededCounts[table], `imported row count for ${table}`);
    }
    await compareAllContent(query, contract, native, successfulPrerequisite.db);
    await operationsChecks({ backend, root: successfulPrerequisite.root, database: successfulPrerequisite.db, query, contract, native, secrets, image: process.env.YOINK_OPS_TEST_IMAGE });
    const nulls = await nativeRequest(native, successfulPrerequisite.db,
      "SELECT payload FROM events WHERE plugin='fixture-edge' ORDER BY id");
    assert.deepEqual(nulls.rows, [['null'], [null]], 'JSON null and SQL NULL remain distinct');
    const wide = await nativeRequest(native, successfulPrerequisite.db,
      "SELECT CAST(id AS TEXT), username FROM users WHERE id=9007199254740993");
    assert.equal(wide.rows[0][0], '9007199254740993');
    assert(wide.rows[0][1].includes('雪') && wide.rows[0][1].includes(rowMarker), 'Unicode and marker values survive import');
    const negativeGroup = await nativeRequest(native, successfulPrerequisite.db,
      'SELECT CAST(id AS TEXT) FROM groups WHERE id=-9007199254740993');
    assert.equal(negativeGroup.rows[0][0], '-9007199254740993', 'negative group identifier survives import');
    for (const table of contract.tables.filter(item => /categories|folders/i.test(item.name))) {
      const parent = table.columns.find(column => column.name.includes('parent'));
      if (!parent) continue;
      const child = await nativeRequest(native, successfulPrerequisite.db,
        `SELECT CAST(id AS TEXT), CAST("${parent.name}" AS TEXT) FROM "${table.name}" WHERE "${parent.name}" IS NOT NULL`);
      assert(BigInt(child.rows[0][1]) > BigInt(child.rows[0][0]), `${table.name} parent ID exceeds child ID`);
    }
    const insight = await nativeRequest(native, successfulPrerequisite.db,
      "SELECT CAST(aliases AS TEXT), CAST(prompt AS TEXT) FROM insight_tldr_aliases");
    assert(insight.ok && insight.rows[0][0].includes('fixture-alias') && insight.rows[0][1].includes('Fixture prompt'),
      'insight alias check values survive import');
    const jsonField = contract.tables.flatMap(table => table.columns.map(column => ({ table: table.name, ...column })))
      .find(column => column.udt === 'json' || column.udt === 'jsonb');
    assert(jsonField, 'legacy contract has a JSON application column');
    const wideJson = await nativeRequest(native, successfulPrerequisite.db,
      `SELECT CAST("${jsonField.name}" AS TEXT) FROM "${jsonField.table}"`);
    assert(wideJson.rows[0][0].includes('9007199254740993'), 'wide JSON number survives exact text conversion');
    assert(wideJson.rows[0][0].includes('123456789012345678901234567890'), 'JSON integer beyond u64 survives as source text');
    const allColumns = contract.tables.flatMap(table => table.columns.map(column => ({ table: table.name, ...column })));
    const timestamp = allColumns.find(column => column.table === 'users' && /timestamp/.test(column.type));
    assert(timestamp, 'users exposes a timestamp for pre-epoch precision fixture');
    const nativeTimestamp = await nativeRequest(native, successfulPrerequisite.db,
      `SELECT CAST("${timestamp.name}" AS TEXT) FROM users WHERE id=9007199254740993`);
    assert.equal(nativeTimestamp.rows[0][0], '-876544', 'pre-epoch timestamp preserves exact signed microseconds');
    const floating = allColumns.find(column => ['float4','float8'].includes(column.udt));
    assert(floating, 'legacy schema exposes a finite floating-point column');
    const nativeFloat = await nativeRequest(native, successfulPrerequisite.db,
      `SELECT "${floating.name}" FROM "${floating.table}"`);
    assert.equal(Number(nativeFloat.rows[0][0]), 9.953947048118591e-11, 'finite floating-point value survives import without rounding');
    const boolean = allColumns.find(column => column.udt === 'bool');
    assert(boolean, 'legacy schema exposes a boolean column');
    const nativeBoolean = await nativeRequest(native, successfulPrerequisite.db,
      `SELECT "${boolean.name}" FROM "${boolean.table}"`);
    assert(['1', 'true'].includes(String(nativeBoolean.rows[0][0]).toLowerCase()), 'true boolean survives import');
    assert(synthetic.emptyWitness, 'synthetic fixture includes an explicit empty text value');
    const empty = await nativeRequest(native, successfulPrerequisite.db,
      `SELECT CAST("${synthetic.emptyWitness.column_name}" AS TEXT) FROM "${synthetic.emptyWitness.table}"`);
    assert.equal(empty.rows[0][0], '', 'empty string survives import');
    let hasNull = false;
    for (const column of allColumns.filter(item => !item.not_null && item.default_value === null)) {
      const result = await nativeRequest(native, successfulPrerequisite.db,
        `SELECT COUNT(*) FROM "${column.table}" WHERE "${column.name}" IS NULL`);
      if (Number(result.rows[0][0]) > 0) { hasNull = true; break; }
    }
    assert(hasNull, 'native import preserves nullable NULL values');
    const afterDigest = await sourceDigests(query, expectedTables);
    assert.deepEqual(afterDigest, beforeDigest, 'source full-content digests remain unchanged by successful import');
    const sequenceValue = await query("SELECT last_value::text FROM api_keys_id_seq");
    assert.equal(sequenceValue, '2000000000', 'source allocator is ahead of imported IDs');
    const nextNative = await nativeRequest(native, successfulPrerequisite.db,
      "INSERT INTO api_keys(name,key_hash,prefix,scopes,created_by,created_at) VALUES ('sequence-check','sequence-hash','p','[]',9007199254740993,0) RETURNING CAST(id AS TEXT)");
    assert(nextNative.ok);
    assert.equal(nextNative.rows[0][0], '2000000001', 'native next internal ID preserves source high-water');
    assert.deepEqual(await sourceCounts(query, expectedTables), seededCounts);
    assert.deepEqual(await sourceDigests(query, expectedTables), beforeDigest);
    assert.equal(Object.keys(countsBefore).length, 41);
    assert.equal(Object.keys(digestsBefore).length, 41);
  } finally { await rm(successfulPrerequisite.root, { recursive: true, force: true }); }

  // Every malformed-source mutation is isolated and restored before the next scenario.
  const revision = await query('SELECT version_num FROM alembic_version');
  const userTimestamp = contract.tables.find(table => table.name === 'users')?.columns.find(column => /timestamp/.test(column.type));
  assert(userTimestamp, 'users timestamp is required for the infinity source case');
  const timestampValue = await query(`SELECT \"${userTimestamp.name}\"::text FROM users WHERE id=9007199254740993`);
  const scenarios = [
    { name: 'wrong revision', mutate: `UPDATE alembic_version SET version_num='0000_wrong_revision'`, restore: `UPDATE alembic_version SET version_num=${quote(revision)}` },
    { name: 'unexpected column', mutate: 'ALTER TABLE users ADD COLUMN import_fixture_extra text', restore: 'ALTER TABLE users DROP COLUMN import_fixture_extra' },
    { name: 'unexpected table', mutate: 'CREATE TABLE import_fixture_extra (id integer)', restore: 'DROP TABLE import_fixture_extra' },
    { name: 'row security hiding source rows', mutate: 'ALTER TABLE events ENABLE ROW LEVEL SECURITY', restore: 'ALTER TABLE events DISABLE ROW LEVEL SECURITY' },
    { name: 'unexpected partitioned table', mutate: 'CREATE TABLE import_fixture_extra (id integer) PARTITION BY RANGE (id)', restore: 'DROP TABLE import_fixture_extra' },
    { name: 'unexpected materialized view', mutate: 'CREATE MATERIALIZED VIEW import_fixture_extra AS SELECT 1 AS id', restore: 'DROP MATERIALIZED VIEW import_fixture_extra' },
    { name: 'duplicate null thread policy defaults', mutate: async () => {
      const first = await query('INSERT INTO thread_policies(group_id) VALUES (-9007199254740993) RETURNING id');
      const second = await query('INSERT INTO thread_policies(group_id) VALUES (-9007199254740993) RETURNING id');
      return `DELETE FROM thread_policies WHERE id IN (${first},${second})`;
    } },
    { name: 'duplicate stats chat/message pair', mutate: async () => {
      const columns = contract.tables.find(table => table.name === 'stats_messages').columns
        .filter(column => column.name !== 'id' && !column.sequence && !column.generated).map(column => `\"${column.name}\"`).join(',');
      const id = await query(`INSERT INTO stats_messages (${columns}) SELECT ${columns} FROM stats_messages LIMIT 1 RETURNING id`);
      return `DELETE FROM stats_messages WHERE id=${id}`;
    } },
    { name: 'infinite timestamp', mutate: async () => {
      await query(`UPDATE users SET \"${userTimestamp.name}\"='infinity' WHERE id=9007199254740993`);
      return `UPDATE users SET \"${userTimestamp.name}\"=${quote(timestampValue)}::${userTimestamp.type.includes('with time zone') ? 'timestamptz' : 'timestamp'} WHERE id=9007199254740993`;
    } },
  ];
  for (const scenario of scenarios) {
    const target = await disposable();
    let restore = scenario.restore;
    try {
      if (typeof scenario.mutate === 'function') restore = await scenario.mutate();
      else await query(scenario.mutate);
      const mutatedDigest = await sourceDigests(query, expectedTables);
      await failure(() => runImport(sourceUrl, target.db, { loopback: true }), scenario.name);
      await missingTarget(target.db);
      assert.deepEqual(await sourceDigests(query, expectedTables), mutatedDigest, `${scenario.name}: importer leaves malformed source content untouched`);
      if (restore) await query(restore);
    } finally {
      if (restore) await query(restore).catch(() => {});
      await rm(target.root, { recursive: true, force: true });
    }
  }

  const args = await disposable();
  try {
    for (const [label, argsList, source] of [
      ['no args', [], 'postgresql://fake:fake@127.0.0.1:1/fake'],
      ['missing destination', ['--destination'], 'postgresql://fake:fake@127.0.0.1:1/fake'],
      ['unknown argument', ['--destination', path.join(args.destinationDir, 'x.db'), '--unknown'], 'postgresql://fake:fake@127.0.0.1:1/fake'],
    ]) await failure(() => run(importer, argsList, { cwd: backend, env: { PATH: process.env.PATH, YOINK_IMPORT_SOURCE_URL: source }, timeout: 10_000 }), label);
    await failure(() => run(importer, ['--destination', args.db], { cwd: backend,
      env: { PATH: process.env.PATH, YOINK_IMPORT_SOURCE_URL: '' }, timeout: 10_000 }), 'missing source URL');
    const defaultTls = path.join(args.destinationDir, 'tls.db');
    await failure(() => runImport(sourceUrl, defaultTls), 'TLS required by default');
    const remote = path.join(args.destinationDir, 'remote.db');
    const numericRemote = 'postgresql://fake:fake@192.0.2.1:5432/fake?sslmode=disable';
    const remoteFailure = await failure(() => runImport(numericRemote, remote, { loopback: true, timeout: 8_000 }), 'non-loopback plaintext rejected by policy');
    assert.notEqual(remoteFailure.category, 'timeout', 'numeric remote rejection is prompt policy enforcement');
    const hostaddr = 'postgresql://fake:fake@localhost:5432/fake?hostaddr=192.0.2.1&sslmode=disable';
    const hostFailure = await failure(() => runImport(hostaddr, path.join(args.destinationDir, 'hostaddr.db'), { loopback: true, timeout: 8_000 }), 'remote hostaddr override rejected');
  } finally { await rm(args.root, { recursive: true, force: true }); }

  const destinations = await disposable();
  try {
    const content = Buffer.from('existing-target-sentinel');
    for (const suffix of ['-wal', '-shm', '.yoink-lock']) {
      const candidate = path.join(destinations.destinationDir, `absent-${suffix.replaceAll(/[^a-z]/g, '')}.db`);
      await writeFile(candidate + suffix, content, { mode: 0o600 });
      await failure(() => runImport(sourceUrl, candidate, { loopback: true }), `pre-existing ${suffix} sidecar`);
      await missingTarget(candidate);
      assert.deepEqual(await readFile(candidate + suffix), content);
    }
    const link = path.join(destinations.destinationDir, 'link.db');
    const linked = path.join(destinations.destinationDir, 'symlink-target.db');
    await writeFile(linked, content, { mode: 0o600 });
    await symlink(linked, link);
    await failure(() => runImport(sourceUrl, link, { loopback: true }), 'symlink destination');
    assert((await lstat(link)).isSymbolicLink());
    assert.deepEqual(await readFile(linked), content);
    const insecure = path.join(destinations.root, 'insecure');
    await mkdir(insecure, { mode: 0o700 });
    await chmod(insecure, 0o777);
    const insecureDb = path.join(insecure, 'out.db');
    await failure(() => runImport(sourceUrl, insecureDb, { loopback: true }), 'insecure parent');
    await missingTarget(insecureDb);
  } finally { await rm(destinations.root, { recursive: true, force: true }); }

  await snapshotChecks({ fixture, sourceUrl, disposable, importer, native, backend, secrets });
  await tlsChecks({ fixture, sourceUrl, disposable, importer, native, backend, secrets, image });
  await edgeChecks({ fixture, runImport, sourceUrl, disposable, native, secrets, contract });
  console.log('Importer acceptance checks completed.');
});

async function sourceCounts(query, tables) {
  const selects = tables.map(table => `SELECT ${quote(table)} AS name, count(*) AS value FROM "${table.replaceAll('"', '""')}"`);
  return JSON.parse(await query(`SELECT json_object_agg(name,value)::text FROM (${selects.join(' UNION ALL ')}) counts`));
}
async function sourceDigests(query, tables) {
  const selects = tables.map(table => `SELECT ${quote(table)} AS name, COALESCE(md5(string_agg(row_text, E'\\n' ORDER BY row_text)),md5('')) AS value FROM
    (SELECT row_to_json(t)::text AS row_text FROM "${table.replaceAll('"', '""')}" t) rows`);
  return JSON.parse(await query(`SELECT json_object_agg(name,value)::text FROM (${selects.join(' UNION ALL ')}) digests`));
}
