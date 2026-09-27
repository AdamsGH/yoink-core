UPDATE yoink_runtime.operations
SET status = CASE WHEN status = 'queued' THEN 'cancelled' ELSE status END,
    cancel_requested = TRUE, updated_at = clock_timestamp()
WHERE actor_id = $1 AND id = $2 AND status IN ('queued', 'running', 'delivering');