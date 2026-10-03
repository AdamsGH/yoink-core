UPDATE api_keys
SET last_used_at = ?1
WHERE key_hash = ?2;
