SELECT id, username, first_name, photo_url, role, language, theme, ban_until, created_at, updated_at,
       CASE WHEN role = 'banned' OR (ban_until IS NOT NULL AND ban_until > CAST(unixepoch('subsec') * 1000000 AS INTEGER)) THEN 1 ELSE 0 END AS blocked
FROM users WHERE id = ?1;
