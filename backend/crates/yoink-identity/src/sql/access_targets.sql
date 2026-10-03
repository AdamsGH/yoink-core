SELECT user_id, chat_id FROM (
    SELECT id AS user_id, id AS chat_id FROM users WHERE id > 0
    UNION ALL
    SELECT u.id AS user_id, g.id AS chat_id
    FROM users u CROSS JOIN groups g WHERE u.id > 0 AND g.id < 0
)
WHERE user_id > ?1 OR (user_id = ?1 AND chat_id > ?2)
ORDER BY user_id, chat_id
LIMIT 128;
