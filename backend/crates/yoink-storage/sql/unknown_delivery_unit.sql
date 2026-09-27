WITH locked AS MATERIALIZED (
    SELECT * FROM yoink_runtime.operations
    WHERE id=$1 AND attempt=$2 AND lease_token=$3 AND worker_id=$4
      AND status='delivering' AND lease_expires_at > clock_timestamp() FOR UPDATE
), marked AS (
    UPDATE yoink_runtime.delivery_units du SET status='unknown', updated_at=clock_timestamp()
    FROM locked l WHERE du.operation_id=l.id AND du.attempt=l.attempt AND du.ordinal=$5 AND du.status='sending'
    RETURNING du.operation_id
)
UPDATE yoink_runtime.operations op SET status='delivery_unknown', worker_id=NULL, lease_token=NULL,
    lease_expires_at=NULL, updated_at=clock_timestamp()
FROM marked WHERE op.id=marked.operation_id;