SELECT id, name, prefix, scopes, created_by, last_used_at, expires_at, revoked, created_at
FROM api_keys
WHERE key_hash = ?1;
