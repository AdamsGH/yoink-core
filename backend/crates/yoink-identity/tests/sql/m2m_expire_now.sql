UPDATE api_keys
SET expires_at = (SELECT CAST((julianday('now') - 2440587.5) * 86400000000 AS INTEGER))
WHERE id = ?1;
