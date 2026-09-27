WITH expired AS MATERIALIZED (
    SELECT id, attempt FROM yoink_runtime.operations
    WHERE status IN ('running', 'delivering') AND lease_expires_at <= clock_timestamp()
    ORDER BY lease_expires_at, id FOR UPDATE SKIP LOCKED LIMIT 100
), units AS (
    UPDATE yoink_runtime.delivery_units du SET status='unknown', updated_at=clock_timestamp()
    FROM expired e WHERE du.operation_id=e.id AND du.attempt=e.attempt AND du.status='sending'
    RETURNING du.operation_id
)
UPDATE yoink_runtime.operations op
SET status = CASE
        WHEN op.status='delivering' AND EXISTS (SELECT 1 FROM units u WHERE u.operation_id=op.id) THEN 'delivery_unknown'
        WHEN op.status='delivering' AND op.delivered_artifact_count > 0 THEN 'delivery_partial'
        WHEN op.status='delivering' AND op.planned_artifact_count IS NULL THEN 'delivery_unknown'
        WHEN op.cancel_requested THEN 'cancelled'
        WHEN op.attempt >= op.max_attempts THEN 'failed'
        ELSE 'queued' END,
    available_at = CASE WHEN (op.status='running' OR (op.status='delivering' AND op.delivered_artifact_count=0
                            AND NOT EXISTS (SELECT 1 FROM units u WHERE u.operation_id=op.id)
                            AND op.planned_artifact_count IS NOT NULL))
                           AND NOT op.cancel_requested AND op.attempt < op.max_attempts
                        THEN clock_timestamp() + (op.retry_delay_secs * INTERVAL '1 second') ELSE op.available_at END,
    worker_id=NULL, lease_token=NULL, lease_expires_at=NULL, progress_percent=NULL, progress_stage=NULL, updated_at=clock_timestamp()
FROM expired e WHERE op.id=e.id;