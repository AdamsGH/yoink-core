import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { nativeRequest } from './native.mjs';

const ident = name => '"' + name.replaceAll('"', '""') + '"';
const quote = text => "'" + text.replaceAll("'", "''") + "'";

// Independent SQL oracle: no importer conversion, ordering or digest code is reused.
export async function compareAllContent(query, contract, native, database) {
  const tables = contract.tables.filter(table => table.name !== 'alembic_version').map(table => ({
    ...table,
    columns: table.columns.filter(column => !(table.name === 'stats_messages' && column.name === 'text_search')),
  }));
  const selects = tables.map(table => {
    const fields = table.columns.map(column => {
      const name = ident(column.name);
      if (column.udt === 'timestamptz') return `(extract(epoch FROM ${name}) * 1000000)::bigint::text`;
      if (column.udt === 'bool') return `${name}::int::text`;
      return `${name}::text`;
    }).join(',');
    return `SELECT ${quote(table.name)} AS name, (SELECT coalesce(json_agg(json_build_array(${fields})), '[]'::json) FROM public.${ident(table.name)}) AS rows`;
  });
  const expected = JSON.parse(await query(`SELECT json_object_agg(name,rows)::text FROM (${selects.join(' UNION ALL ')}) oracle`));
  let cells = 0;
  for (const table of tables) {
    const fields = table.columns.map(column => ['float4', 'float8'].includes(column.udt)
      ? ident(column.name) : `CAST(${ident(column.name)} AS TEXT)`).join(',');
    const actual = await nativeRequest(native, database, `SELECT ${fields} FROM ${ident(table.name)}`);
    assert.equal(digest(actual.rows, table.columns), digest(expected[table.name], table.columns),
      `independent full-content comparison for ${table.name}`);
    cells += actual.rows.length * table.columns.length;
  }
  console.log(`Independent source/native comparison: ${tables.length} tables, ${cells} cells.`);
}

function digest(rows, columns) {
  const normalized = rows.map(row => JSON.stringify(row.map((value, index) =>
    value !== null && ['float4', 'float8'].includes(columns[index].udt) ? Number(value) : value))).sort();
  return createHash('sha256').update(JSON.stringify(normalized)).digest('hex');
}
