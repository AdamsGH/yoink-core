SELECT SUM(user_id=?1 AND is_pool=1) AS pool,
       SUM(user_id=?2) AS foreign_count,
       SUM(user_id=?1) AS own
FROM cookies;
