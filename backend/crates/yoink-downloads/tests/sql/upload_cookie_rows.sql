SELECT id,user_id,domain,content,is_pool,created_at,updated_at
FROM cookies WHERE user_id IN (?1,?2) AND LOWER(TRIM(domain,'.'))=?3 ORDER BY id;
