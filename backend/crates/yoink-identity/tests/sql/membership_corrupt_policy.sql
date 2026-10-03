INSERT INTO bot_settings (key,value,updated_at) VALUES ('bot_access_mode','corrupt',10) ON CONFLICT(key) DO UPDATE SET value=excluded.value;
