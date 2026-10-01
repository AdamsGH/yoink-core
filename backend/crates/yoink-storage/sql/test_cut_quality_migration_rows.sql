SELECT actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected
FROM runtime_cut_sessions
ORDER BY actor_id;
