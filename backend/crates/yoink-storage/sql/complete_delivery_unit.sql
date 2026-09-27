WITH locked AS MATERIALIZED (
    SELECT * FROM yoink_runtime.operations
    WHERE id=$1 AND attempt=$2 AND lease_token=$3 AND worker_id=$4
      AND status='delivering' AND lease_expires_at > clock_timestamp() FOR UPDATE
), unit AS (
    UPDATE yoink_runtime.delivery_units du SET status='sent', message_ids=$6::integer[], updated_at=clock_timestamp()
    FROM locked l WHERE du.operation_id=l.id AND du.attempt=l.attempt AND du.ordinal=$5
      AND du.status='sending' AND cardinality($6::integer[])=du.artifact_count
      AND (SELECT count(DISTINCT value) FROM unnest($6::integer[]) AS ids(value))=du.artifact_count
      AND NOT EXISTS (SELECT 1 FROM unnest($6::integer[]) AS ids(value) WHERE value <= 0)
    RETURNING du.operation_id, du.attempt, du.artifact_count
)
UPDATE yoink_runtime.operations op SET
    delivered_artifact_count=op.delivered_artifact_count+unit.artifact_count,
    delivered_message_id=CASE WHEN op.delivered_artifact_count+unit.artifact_count=op.planned_artifact_count THEN $7 ELSE NULL END,
    status=CASE WHEN op.delivered_artifact_count+unit.artifact_count=op.planned_artifact_count THEN 'succeeded' ELSE 'delivering' END,
    worker_id=CASE WHEN op.delivered_artifact_count+unit.artifact_count=op.planned_artifact_count THEN NULL ELSE op.worker_id END,
    lease_token=CASE WHEN op.delivered_artifact_count+unit.artifact_count=op.planned_artifact_count THEN NULL ELSE op.lease_token END,
    lease_expires_at=CASE WHEN op.delivered_artifact_count+unit.artifact_count=op.planned_artifact_count THEN NULL ELSE op.lease_expires_at END,
    updated_at=clock_timestamp()
FROM unit WHERE op.id=unit.operation_id AND op.attempt=unit.attempt;