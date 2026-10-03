SELECT key_hash, scopes, revoked, expires_at
FROM api_keys
WHERE key_hash = ?1;
