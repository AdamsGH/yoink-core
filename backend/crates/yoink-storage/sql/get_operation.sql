SELECT id, actor_id, status, attempt, max_attempts, cancel_requested, created_at, updated_at
FROM yoink_runtime.operations
WHERE actor_id = $1 AND id = $2;
