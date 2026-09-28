import { readFile, writeFile } from 'node:fs/promises';
import { withLegacyFixture } from './legacy-fixture.mjs';

if (process.argv.length !== 2) throw new Error('no arguments accepted');
const sql = await readFile(new URL('./sql/legacy-catalog.sql', import.meta.url), 'utf8');
await withLegacyFixture(async ({ query }) => {
  const schema = JSON.parse(await query(sql));
  await writeFile(new URL('../schema/legacy-postgres.json', import.meta.url), `${JSON.stringify(schema, null, 2)}\n`, { mode: 0o600 });
  console.log(`Saved schema-only oracle: ${schema.tables.length} tables, revision ${schema.revision}, schema/legacy-postgres.json`);
  const types = [...new Set(schema.tables.flatMap(table => table.columns.map(column => column.type)))];
  console.log(`Types: ${types.sort().join(', ')}`);
});
