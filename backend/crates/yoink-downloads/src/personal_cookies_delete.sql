DELETE FROM cookies WHERE user_id=?1 AND is_pool=0
AND (?2 IS NULL OR LOWER(TRIM(domain,'.'))=?2);
