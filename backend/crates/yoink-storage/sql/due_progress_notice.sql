SELECT operation_id,actor_id,bot_id,chat_id,message_id,language,last_text
FROM runtime_progress_notices
WHERE bot_id=?1 AND finished=0 AND next_attempt_at<=?2
ORDER BY next_attempt_at,operation_id LIMIT 1;
