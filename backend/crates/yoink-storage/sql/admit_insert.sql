INSERT INTO yoink_runtime.operations (actor_id, idempotency_key, request, max_attempts)
VALUES ($1, $2, $3, $4)
ON CONFLICT (actor_id, idempotency_key) DO NOTHING
RETURNING id;
