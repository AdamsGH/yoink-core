SELECT json_object('actor_id',actor_id,'bot_id',bot_id,'chat_id',chat_id,'id',id,'revision',revision,'stage',stage,'origin_message_id',origin_message_id,'last_message_id',last_message_id,'request_json',request_json,'selected',selected,'operation_id',operation_id) AS snapshot
FROM runtime_quality_sessions ORDER BY actor_id;
