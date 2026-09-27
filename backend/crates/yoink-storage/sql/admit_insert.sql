INSERT INTO yoink_runtime.operations (actor_id, idempotency_key, request, max_attempts, retry_delay_secs, cache_key)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (actor_id, idempotency_key) DO NOTHING
RETURNING id;