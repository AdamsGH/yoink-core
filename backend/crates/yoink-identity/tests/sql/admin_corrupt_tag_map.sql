INSERT INTO bot_settings (key, value, updated_at) VALUES ('tag_map', 'not-json', 1700000000000000) ON CONFLICT(key) DO UPDATE SET value = excluded.value;
