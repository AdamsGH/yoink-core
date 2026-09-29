INSERT INTO runtime_personal_cookie_mutation_watermarks(actor_id,message_id) VALUES(?1,?2)
ON CONFLICT(actor_id) DO UPDATE SET message_id=excluded.message_id
WHERE runtime_personal_cookie_mutation_watermarks.message_id<excluded.message_id
RETURNING actor_id;
