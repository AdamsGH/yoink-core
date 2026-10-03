INSERT INTO bot_settings (key, value, updated_at)
VALUES ('bot_access_mode', ?1, CAST(unixepoch('subsec') * 1000000 AS INTEGER))
ON CONFLICT (key) DO UPDATE SET value = excluded.value;
