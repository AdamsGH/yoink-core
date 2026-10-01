SELECT json_object('actor_id',actor_id,'bot_id',bot_id,'chat_id',chat_id,'id',id,'revision',revision,'stage',stage,'origin_message_id',origin_message_id,'last_message_id',last_message_id,'source_url',source_url,'begin_start',begin_start,'start',start,'end',end,'operation_id',operation_id,'selected',selected) AS snapshot
FROM runtime_cut_sessions ORDER BY actor_id;
