WITH expired AS (
    SELECT id FROM yoink_runtime.operations
    WHERE status IN ('running', 'delivering') AND lease_expires_at <= clock_timestamp()
    ORDER BY lease_expires_at, id FOR UPDATE SKIP LOCKED LIMIT 100
)
UPDATE yoink_runtime.operations AS op
SET status = CASE WHEN op.status = 'delivering' THEN 'delivery_unknown'
                  WHEN op.cancel_requested THEN 'cancelled'
                  WHEN op.attempt >= op.max_attempts THEN 'failed'
                  ELSE 'queued' END,
    available_at = CASE WHEN op.status = 'running' AND NOT op.cancel_requested AND op.attempt < op.max_attempts
                        THEN clock_timestamp() + (op.retry_delay_secs * INTERVAL '1 second') ELSE op.available_at END,
    worker_id = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = clock_timestamp()
FROM expired WHERE op.id = expired.id;