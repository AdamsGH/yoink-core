SELECT id AS dirty_id, id, message_id, chat_id, date, from_user, text, caption, msg_type
FROM stats_messages WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2;
