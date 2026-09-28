INSERT INTO runtime_migrations (version, checksum, schema_hash, success, installed_at)
SELECT ?1, ?2, schema_hash, 1, installed_at FROM runtime_migrations ORDER BY version DESC LIMIT 1;
