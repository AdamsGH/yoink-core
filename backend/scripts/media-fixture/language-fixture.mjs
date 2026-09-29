export async function withRussianPreference(fixtureSql, run) {
  const saved = await fixtureSql("SELECT json_object('language',language,'updated_at',updated_at) FROM users WHERE id=42");
  try {
    await fixtureSql("UPDATE users SET language='ru' WHERE id=42");
    return await run();
  } finally {
    const row = JSON.parse(saved);
    const literal = value => value === null ? 'NULL' : typeof value === 'number' ? String(value) : `'${String(value).replaceAll("'", "''")}'`;
    await fixtureSql(`UPDATE users SET language=${literal(row.language)},updated_at=${literal(row.updated_at)} WHERE id=42`);
  }
}
