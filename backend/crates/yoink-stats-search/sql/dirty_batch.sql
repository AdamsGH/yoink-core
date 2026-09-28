SELECT d.id AS dirty_id, m.id, m.message_id, m.chat_id, m.date,
       m.from_user, m.text, m.caption, m.msg_type
FROM stats_search_dirty d LEFT JOIN stats_messages m ON m.id = d.id
WHERE (?1 IS NULL OR d.id > ?1) ORDER BY d.id LIMIT ?2;
