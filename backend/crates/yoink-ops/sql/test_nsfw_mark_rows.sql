SELECT json_object('actor',actor_id,'arm',1) AS snapshot FROM runtime_nsfw_marks
UNION ALL
SELECT json_object('actor',actor_id,'bot',bot_id,'chat',chat_id,'message',message_id) AS snapshot FROM runtime_nsfw_mark_events
ORDER BY snapshot;
