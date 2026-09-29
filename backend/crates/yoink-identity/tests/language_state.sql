SELECT username, first_name, role, language, theme, ban_until, created_at, updated_at, is_premium, photo_url
FROM users WHERE id = ?1;
