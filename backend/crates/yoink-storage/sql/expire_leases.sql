WITH expired AS (
    SELECT id
    FROM yoink_runtime.operations
    WHERE status = 'running' AND lease_expires_at <= clock_timestamp()
    ORDER BY lease_expires_at, id
    FOR UPDATE SKIP LOCKED
    LIMIT 100
)
UPDATE yoink_runtime.operations AS op
SET status = CASE WHEN op.cancel_requested THEN 'cancelled'
                  WHEN op.attempt >= op.max_attempts THEN 'failed'
                  ELSE 'queued' END,
    worker_id = NULL,
    lease_token = NULL,
    lease_expires_at = NULL,
    updated_at = clock_timestamp()
FROM expired
WHERE op.id = expired.id;
