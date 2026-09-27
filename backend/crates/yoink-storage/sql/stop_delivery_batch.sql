WITH locked AS MATERIALIZED (
    SELECT * FROM yoink_runtime.operations
    WHERE id=$1 AND attempt=$2 AND lease_token=$3 AND worker_id=$4
      AND status='delivering' AND lease_expires_at > clock_timestamp()
      AND ($5 <> 'cancel' OR cancel_requested)
    FOR UPDATE
), pending AS (
    UPDATE yoink_runtime.delivery_units du SET status='unknown', updated_at=clock_timestamp()
    FROM locked l WHERE du.operation_id=l.id AND du.attempt=l.attempt AND du.status='sending'
    RETURNING du.operation_id
)
UPDATE yoink_runtime.operations op SET
    status=CASE WHEN EXISTS (SELECT 1 FROM pending p WHERE p.operation_id=op.id) THEN 'delivery_unknown'
                WHEN op.delivered_artifact_count > 0 THEN 'delivery_partial'
                WHEN op.cancel_requested THEN 'cancelled'
                WHEN $5='cancel' THEN 'cancelled'
                WHEN $5='denied' THEN 'failed'
                WHEN op.attempt < op.max_attempts THEN 'queued' ELSE 'failed' END,
    available_at=CASE WHEN op.delivered_artifact_count=0 AND NOT op.cancel_requested AND $5='shutdown' AND op.attempt < op.max_attempts
                      THEN clock_timestamp()+(op.retry_delay_secs*INTERVAL '1 second') ELSE op.available_at END,
    worker_id=NULL, lease_token=NULL, lease_expires_at=NULL,
    progress_percent=NULL, progress_stage=NULL, updated_at=clock_timestamp()
FROM locked l WHERE op.id=l.id;