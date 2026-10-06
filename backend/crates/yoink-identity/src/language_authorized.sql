SELECT 1 FROM users WHERE id = ?1
AND (role IN ('user', 'moderator', 'admin', 'owner') OR (?2 = 1 AND role = 'restricted'))
AND (ban_until IS NULL OR ban_until <= CAST(unixepoch('subsec') * 1000000 AS INTEGER));
