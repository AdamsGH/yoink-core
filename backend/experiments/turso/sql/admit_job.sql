INSERT INTO jobs(actor_id, request_key, payload)
VALUES (?1, ?2, ?3)
ON CONFLICT(actor_id, request_key) DO NOTHING;
