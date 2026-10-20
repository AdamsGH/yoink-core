INSERT INTO bot_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at;
