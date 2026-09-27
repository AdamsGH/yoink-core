INSERT INTO yoink_runtime.operations (actor_id, idempotency_key, request, max_attempts, retry_delay_secs)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (actor_id, idempotency_key) DO NOTHING
RETURNING id;