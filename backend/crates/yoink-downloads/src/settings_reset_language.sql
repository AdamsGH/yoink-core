UPDATE users SET language = 'en', updated_at = max(updated_at + 1, ?2)
WHERE id = ?1 AND updated_at >= 0 AND updated_at < 9223372036854775807
RETURNING id;
