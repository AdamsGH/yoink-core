SELECT 1 FROM users WHERE id = ?1 AND ?3 = 1
AND role IN ('user', 'restricted', 'moderator', 'admin', 'owner')
AND (ban_until IS NULL OR ban_until <= ?2);
