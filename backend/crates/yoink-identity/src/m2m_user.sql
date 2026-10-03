SELECT id, username, first_name, role, language, created_at, ban_until
FROM users
WHERE id = ?1;
