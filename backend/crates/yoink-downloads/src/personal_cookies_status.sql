SELECT CASE WHEN is_valid=1 AND length(CAST(content AS BLOB))<=?3 THEN content END AS content,
       CASE WHEN length(CAST(content AS BLOB))>?3 THEN 1 ELSE 0 END AS too_large
FROM cookies
WHERE user_id=?1 AND is_pool=0 AND LOWER(TRIM(domain,'.'))=?2
ORDER BY id LIMIT 2;
