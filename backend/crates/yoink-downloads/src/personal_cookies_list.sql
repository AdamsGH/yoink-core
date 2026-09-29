SELECT id, CASE WHEN length(CAST(domain AS BLOB))<=255 THEN domain END AS domain
FROM cookies
WHERE user_id=?1 AND is_pool=0 AND is_valid=1 AND id>?2
ORDER BY id LIMIT 11;
