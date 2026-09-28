SELECT id, message_id, chat_id, date, from_user, text, caption, msg_type
FROM stats_messages WHERE id = ?1;