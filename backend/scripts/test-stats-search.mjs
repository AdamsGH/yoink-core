import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { withLegacyFixture } from './legacy-fixture.mjs';

const base = new URL('./stats-search-fixture/', import.meta.url);
const corpus = JSON.parse(await readFile(new URL('cases.json', base), 'utf8'));
const template = await readFile(new URL('oracle.sql', base), 'utf8');
const literal = `'${JSON.stringify(corpus).replaceAll("'", "''")}'`;
const sql = template.replaceAll('__CORPUS__', literal);
await withLegacyFixture(async ({ query }) => {
  const actual = JSON.parse(await query(sql));
  assert.equal(actual.length, corpus.queries.length);
  const mismatches = [];
  for (const [index, result] of actual.entries()) {
    const expected = corpus.queries[index];
    assert.equal(result.label, expected.label);
    if (JSON.stringify(result.ids) !== JSON.stringify(expected.postgres)) {
      mismatches.push({ label: result.label, expected: expected.postgres, actual: result.ids });
    }
  }
  assert.deepEqual(mismatches, [], 'Legacy FTS contract changed; inspect actual PostgreSQL evidence');
  const changed = corpus.queries.filter(item => JSON.stringify(item.postgres) !== JSON.stringify(item.tantivy)).length;
  console.log(`Stats search oracle: ${corpus.messages.length} source messages, ${actual.length} legacy queries verified, ${changed} explicit Tantivy differences.`);
});
