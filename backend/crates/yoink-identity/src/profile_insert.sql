INSERT INTO users (id, username, first_name, photo_url, role, language, theme, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, 'en', 'macchiato', CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000)
ON CONFLICT(id) DO NOTHING;
