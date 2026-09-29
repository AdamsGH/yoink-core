UPDATE users
SET language = ?1,
    updated_at = max(updated_at + 1, ?5)
WHERE id = ?2 AND ?4 = 1 AND updated_at = ?3 AND updated_at >= 0
  AND updated_at < 9223372036854775807
  AND role IN ('user', 'restricted', 'moderator', 'admin', 'owner')
  AND (ban_until IS NULL OR ban_until <= ?5)
RETURNING language, updated_at;
