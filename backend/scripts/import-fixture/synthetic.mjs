const quote = value => "'" + String(value).replaceAll("'", "''") + "'";
const ident = value => '"' + String(value).replaceAll('"', '""') + '"';

export async function seedSynthetic(query, contract) {
  const metadata = JSON.parse(await query(`SELECT json_agg(json_build_object(
    'table', c.table_name, 'column_name', c.column_name, 'type', c.data_type, 'udt', c.udt_name,
    'nullable', c.is_nullable = 'YES', 'default', c.column_default, 'identity', c.is_identity,
    'generated', c.is_generated, 'ordinal', c.ordinal_position,
    'ref_table', fk.foreign_table_name, 'ref_column', fk.foreign_column_name,
    'enum_values', (SELECT array_agg(e.enumlabel ORDER BY e.enumsortorder) FROM pg_type t JOIN pg_enum e ON e.enumtypid=t.oid WHERE t.typname=c.udt_name)
  ) ORDER BY c.table_name,c.ordinal_position)::text
  FROM information_schema.columns c
  LEFT JOIN (
    SELECT kcu.table_name,kcu.column_name,ccu.table_name AS foreign_table_name,ccu.column_name AS foreign_column_name
    FROM information_schema.key_column_usage kcu JOIN information_schema.constraint_column_usage ccu
      ON ccu.constraint_name=kcu.constraint_name AND ccu.constraint_schema=kcu.constraint_schema
    JOIN information_schema.table_constraints tc ON tc.constraint_name=kcu.constraint_name
      AND tc.constraint_schema=kcu.constraint_schema AND tc.constraint_type='FOREIGN KEY'
    WHERE kcu.table_schema='public'
  ) fk ON fk.table_name=c.table_name AND fk.column_name=c.column_name
  WHERE c.table_schema='public' AND c.table_name <> 'alembic_version'`));
  const tables = contract.tables.filter(t => t.name !== 'alembic_version').map(t => t.name);
  const byTable = new Map(tables.map(name => [name, metadata.filter(c => c.table === name)]));
  const order = [];
  const pending = new Set(tables);
  while (pending.size) {
    const ready = [...pending].filter(name => (byTable.get(name) ?? []).every(c =>
      c.nullable || !c.ref_table || c.ref_table === name || !pending.has(c.ref_table)));
    if (!ready.length) throw new Error(`required foreign-key cycle cannot be seeded deterministically: ${[...pending].join(',')}`);
    for (const name of ready) { order.push(name); pending.delete(name); }
  }
  const ids = new Map(tables.map((name, i) => [name, 1000 + i]));
  const emptyWitness = metadata.find(c => c.nullable && c.default === null && !c.ref_table && ['text','character varying','character'].includes(c.type));
  for (const table of order) {
    const cols = byTable.get(table);
    const values = new Map();
    for (const c of cols) {
      if (c.generated !== 'NEVER' || c.identity === 'YES') continue;
      if (c.column_name === 'id' && /^(int2|int4|int8)$/.test(c.udt)) {
        values.set(c.column_name, table === 'users' ? '9007199254740993' : table === 'groups' ? '-9007199254740993' : String(ids.get(table)));
      } else if (table === 'users' && c.column_name === 'username') {
        values.set(c.column_name, literal(table, c));
      } else if (c.ref_table && !c.nullable && ids.has(c.ref_table)) {
        values.set(c.column_name, c.ref_table === 'users' ? '9007199254740993' : c.ref_table === 'groups' ? '-9007199254740993' : String(ids.get(c.ref_table)));
      } else if (c.default === null && !c.nullable) {
        values.set(c.column_name, literal(table, c));
      } else if (emptyWitness?.table === table && emptyWitness.column_name === c.column_name) {
        values.set(c.column_name, quote(''));
      } else if (c.udt === 'json' || c.udt === 'jsonb' || /^(timestamp|timestamptz|float4|float8|bool)$/.test(c.udt)
          || table === 'insight_tldr_aliases' && ['aliases','prompt','target_alias','domains'].includes(c.column_name)) {
        values.set(c.column_name, literal(table, c));
      }
    }
    const names = [...values.keys()];
    if (!names.length) throw new Error(`table ${table} has no insertable columns`);
    const hierarchy = /categories|folders/i.test(table) && cols.find(c => c.ref_table === table && /parent/i.test(c.column_name));
    if (hierarchy) {
      const base = ids.get(table);
      const parentValues = new Map(values);
      const childValues = new Map(values);
      parentValues.set('id', String(base + 1));
      childValues.set('id', String(base));
      parentValues.delete(hierarchy.column_name);
      childValues.set(hierarchy.column_name, String(base + 1));
      parentValues.set('name', quote('Родитель 雪'));
      childValues.set('name', quote('Д О_Ч-Ь 雪'));
      parentValues.set('slug', quote(`fixture-${table}-parent`));
      childValues.set('slug', quote(`fixture-${table}-child`));
      for (const row of [parentValues, childValues]) {
        const rowNames = [...row.keys()];
        await query(`INSERT INTO ${ident(table)} (${rowNames.map(ident).join(',')}) VALUES (${rowNames.map(n => row.get(n)).join(',')})`);
      }
    } else {
      await query(`INSERT INTO ${ident(table)} (${names.map(ident).join(',')}) VALUES (${names.map(n => values.get(n)).join(',')})`);
    }
  }
  await query(`INSERT INTO events (id,plugin,event_type,payload,created_at) VALUES
    (2001,'fixture-edge','json-null','null'::json,'2024-02-03 04:05:06.123456+00'),
    (2002,'fixture-edge','sql-null',NULL,'2024-02-03 07:05:06.123456+03')`);
  await query(`SELECT setval(pg_get_serial_sequence('api_keys','id'), 2000000000, true)`);
  return { tables, ids, emptyWitness };
}

function literal(table, c) {
  const col = c.column_name.toLowerCase();
  const type = c.udt;
  if (c.enum_values?.length) return quote(c.enum_values[0]);
  if (table === 'insight_tldr_aliases') {
    if (col === 'aliases') return type === 'json' || type === 'jsonb' ? `'["fixture-alias"]'::${type}` : c.type === 'ARRAY' ? `ARRAY['fixture-alias']::${type.slice(1)}[]` : quote('fixture-alias');
    if (col === 'prompt') return quote('Fixture prompt');
    if (col === 'target_alias') return quote('fixture-target');
    if (col === 'domains') return type === 'json' || type === 'jsonb' ? `'["fixture.invalid"]'::${type}` : c.type === 'ARRAY' ? `ARRAY['fixture.invalid']::${type.slice(1)}[]` : quote('fixture.invalid');
  }
  if (type === 'bool') return 'TRUE';
  if (type === 'json' || type === 'jsonb') return `'${'{\"fixture\":\"ok\",\"wide\":9007199254740993,\"huge\":123456789012345678901234567890}'}'::${type}`;
  if (type === 'timestamp' || type === 'timestamptz') return `'${table === 'users' ? '1969-12-31 23:59:59.123456+00' : '2024-02-03 04:05:06.123456+00'}'::${type}`;
  if (type === 'date') return `'2024-02-03'::date`;
  if (type === 'time' || type === 'timetz') return `'04:05:06.123456+00'::${type}`;
  if (type === 'float4' || type === 'float8') return '9.953947048118591e-11';
  if (type === 'numeric' || type === 'int2' || type === 'int4' || type === 'int8') return '1';
  if (type === 'uuid') return quote('00000000-0000-4000-8000-000000000001');
  if (type === 'bytea') return `decode('01','hex')`;
  if (c.type === 'ARRAY') return `ARRAY[]::${type.slice(1)}[]`;
  if (col.includes('name') || col.includes('title') || col.includes('label')) {
    if (table === 'users' && col === 'username') return quote(`IMPORT_FIXTURE_ROW_MARKER_9c21-雪`);
    return quote(`fixture-${table}-雪`);
  }
  if (col.includes('alias')) return quote('fixture-alias');
  if (col.includes('domain')) return quote('fixture.invalid');
  if (col.includes('prompt')) return quote('Fixture prompt');
  if (col.includes('url')) return quote('https://fixture.invalid/item');
  if (col.includes('email')) return quote('fixture@example.invalid');
  if (col.includes('role')) return quote('user');
  if (col.includes('status')) return quote('active');
  if (col.includes('description') || col.includes('content') || col.includes('text') || col.includes('message')) return quote('IMPORT_FIXTURE_ROW_MARKER_9c21');
  if (/^(smallint|integer|bigint)$/.test(type)) return '1';
  return quote('fixture');
}
