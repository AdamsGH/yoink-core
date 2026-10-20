INSERT INTO runtime_progress_notices(operation_id,actor_id,bot_id,chat_id,message_id,language)
SELECT id,actor_id,?3,?4,?5,?6 FROM runtime_operations
WHERE id=?1 AND actor_id=?2
 AND json_extract(request,'$.target.bot_id')=?3
 AND json_extract(request,'$.target.chat_id')=?4
ON CONFLICT(operation_id) DO UPDATE SET
 message_id=excluded.message_id,language=excluded.language,last_text=NULL,finished=0,
 next_attempt_at=CAST(unixepoch('subsec') * 1000000 AS INTEGER)
WHERE runtime_progress_notices.last_text=?7
 AND runtime_progress_notices.actor_id=excluded.actor_id
 AND runtime_progress_notices.bot_id=excluded.bot_id
 AND runtime_progress_notices.chat_id=excluded.chat_id
 AND runtime_progress_notices.message_id<>excluded.message_id
RETURNING operation_id;
