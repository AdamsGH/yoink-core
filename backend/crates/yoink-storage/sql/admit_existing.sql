SELECT id, request
FROM yoink_runtime.operations
WHERE actor_id = $1 AND idempotency_key = $2;
