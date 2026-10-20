INSERT INTO runtime_progress_notices(operation_id,actor_id,bot_id,chat_id,message_id,language,last_text)
SELECT id,actor_id,
 json_extract(request,'$.target.bot_id'),
 json_extract(request,'$.target.chat_id'),
 json_extract(request,'$.target.reply_to_message_id'),
 'en',?2
FROM runtime_operations
WHERE id=?1
 AND json_extract(request,'$.target.chat_id')=actor_id
 AND json_extract(request,'$.target.thread_id') IS NULL
 AND json_extract(request,'$.target.reply_to_message_id')>0
ON CONFLICT(operation_id) DO NOTHING;
