INSERT INTO bot_settings (key, value, updated_at)
VALUES ('insight_byok_enabled', ?1, CAST(strftime('%s', 'now') AS INTEGER) * 1000000)
ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at;
