INSERT INTO cookies (user_id, domain, content, is_valid, is_pool, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, 1700000000000000, 1700000000000000) RETURNING id;
