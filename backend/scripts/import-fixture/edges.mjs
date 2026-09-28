import assert from 'node:assert/strict';
import { readdir, rm } from 'node:fs/promises';
import { nativeRequest } from './native.mjs';
import { assertNoSecrets } from './process.mjs';
import { compareAllContent } from './content.mjs';

export async function edgeChecks({ fixture, runImport, sourceUrl, disposable, native, secrets, contract }) {
  // Final fixture phase: this database is disposable and has no remaining consumers.
  await fixture.query("DELETE FROM events; SELECT setval('events_id_seq',20000,false)");
  const target = await disposable();
  try {
    await assert.rejects(runImport(sourceUrl, target.db, { loopback: true, flags: ['--max-row-bytes', '1'] }), error => {
      assert.equal(error.category, 'exit');
      assertNoSecrets(error.stdout + error.stderr, secrets);
      assert(error.stderr.includes('cannot be represented safely'));
      return true;
    });
    assert.deepEqual(await readdir(target.destinationDir), [], 'conversion failure removes staging and reservation');
    await runImport(sourceUrl, target.db, { loopback: true });
    await compareAllContent(fixture.query, contract, native, target.db);
    const allocated = await nativeRequest(native, target.db,
      "INSERT INTO events(plugin,event_type,created_at) VALUES ('fixture','after-import',0) RETURNING CAST(id AS TEXT)");
    assert.equal(allocated.rows[0][0], '20000', 'empty table with is_called=false preserves the next source ID exactly');
    assert.equal(await fixture.query("SELECT last_value::text || '/' || is_called::text FROM events_id_seq"), '20000/false');
    assert.equal(await fixture.query('SELECT count(*) FROM events'), '0', 'native insert does not modify the source');
  } finally { await rm(target.root, { recursive: true, force: true }); }
}
