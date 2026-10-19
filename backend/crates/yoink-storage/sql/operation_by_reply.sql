SELECT o.id
FROM runtime_operations o
LEFT JOIN runtime_progress_notices p ON p.operation_id = o.id
WHERE o.actor_id = ?1
 AND json_extract(o.request, '$.target.bot_id') = ?2
 AND json_extract(o.request, '$.target.chat_id') = ?3
 AND (p.message_id = ?4 OR json_extract(o.request, '$.target.reply_to_message_id') = ?4)
ORDER BY o.created_at DESC, o.id
LIMIT 1;
