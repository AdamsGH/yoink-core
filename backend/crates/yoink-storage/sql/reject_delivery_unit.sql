WITH locked AS MATERIALIZED (
    SELECT * FROM yoink_runtime.operations
    WHERE id=$1 AND attempt=$2 AND lease_token=$3 AND worker_id=$4
      AND status='delivering' AND lease_expires_at > clock_timestamp() FOR UPDATE
), rejected AS (
    UPDATE yoink_runtime.delivery_units du SET status='rejected', updated_at=clock_timestamp()
    FROM locked l WHERE du.operation_id=l.id AND du.attempt=l.attempt AND du.ordinal=$5 AND du.status='sending'
    RETURNING du.operation_id
)
UPDATE yoink_runtime.operations op SET
    status=CASE WHEN op.delivered_artifact_count > 0 THEN 'delivery_partial'
                WHEN op.cancel_requested THEN 'cancelled'
                WHEN $6::integer IS NOT NULL AND op.attempt < op.max_attempts THEN 'queued' ELSE 'failed' END,
    available_at=CASE WHEN op.delivered_artifact_count=0 AND NOT op.cancel_requested AND $6::integer IS NOT NULL AND op.attempt < op.max_attempts
                      THEN clock_timestamp()+(GREATEST(op.retry_delay_secs, $6)::double precision * INTERVAL '1 second') ELSE op.available_at END,
    worker_id=NULL, lease_token=NULL, lease_expires_at=NULL, progress_percent=NULL, progress_stage=NULL, updated_at=clock_timestamp()
FROM rejected WHERE op.id=rejected.operation_id;