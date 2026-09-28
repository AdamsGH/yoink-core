SELECT id, role AS role, language, ban_until
FROM users
WHERE id = ?1;