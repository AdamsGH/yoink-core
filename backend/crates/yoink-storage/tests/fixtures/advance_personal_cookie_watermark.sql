INSERT INTO runtime_personal_cookie_mutation_watermarks(actor_id,message_id) VALUES(42,?1)
ON CONFLICT(actor_id) DO UPDATE SET message_id=excluded.message_id
WHERE runtime_personal_cookie_mutation_watermarks.message_id<excluded.message_id;
