DELETE FROM cookies WHERE user_id=?1 AND is_pool=0
AND LOWER(TRIM(domain,'.'))=?2;
