-- Byte lengths deliberately use BLOB casts (UTF-8 bytes, not Unicode characters).
-- Count physical personal rows conservatively, including malformed imported domains.
SELECT
 COALESCE(SUM(CASE WHEN user_id=?1 THEN 1 ELSE 0 END),0) AS actor_domains,
 COALESCE(SUM(CASE WHEN user_id=?1 THEN length(CAST(content AS BLOB)) ELSE 0 END),0) AS actor_bytes,
 COALESCE(SUM(length(CAST(content AS BLOB))),0) AS global_bytes,
 COALESCE(SUM(CASE WHEN user_id=?1 AND LOWER(TRIM(domain,'.'))=?2 THEN 1 ELSE 0 END),0) AS replaced_domains,
 COALESCE(SUM(CASE WHEN user_id=?1 AND LOWER(TRIM(domain,'.'))=?2 THEN length(CAST(content AS BLOB)) ELSE 0 END),0) AS replaced_bytes
FROM cookies WHERE is_pool=0;
