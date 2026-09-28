SELECT id, user_id, domain,
       CASE WHEN length(CAST(content AS BLOB)) <= ?2 THEN content ELSE NULL END AS content,
       length(CAST(content AS BLOB)) AS content_bytes, is_valid, is_pool, updated_at
FROM cookies
WHERE id = ?1;