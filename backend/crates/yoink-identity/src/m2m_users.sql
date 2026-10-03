SELECT id, username, first_name, role, language, created_at, ban_until
FROM users
WHERE (?1 IS NULL OR role = ?1)
ORDER BY created_at DESC
LIMIT ?2 OFFSET ?3;
