INSERT INTO runtime_progress_notices(operation_id,actor_id,bot_id,chat_id,message_id,language)
SELECT id,actor_id,?3,?4,?5,?6 FROM runtime_operations
WHERE id=?1 AND actor_id=?2
 AND json_extract(request,'$.target.bot_id')=?3
 AND json_extract(request,'$.target.chat_id')=?4
ON CONFLICT(operation_id) DO NOTHING
RETURNING operation_id;
