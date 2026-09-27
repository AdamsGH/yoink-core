INSERT INTO requests(actor_id, request_key) VALUES (?1, ?2) ON CONFLICT(actor_id, request_key) DO NOTHING;
