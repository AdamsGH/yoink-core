WITH locked AS MATERIALIZED (
    SELECT * FROM yoink_runtime.operations
    WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4
      AND status = 'delivering' AND lease_expires_at > clock_timestamp()
    FOR UPDATE
), checks AS (
    SELECT l.id FROM locked l
    WHERE l.planned_artifact_count IS NOT NULL AND NOT l.cancel_requested
      AND $5 = COALESCE((SELECT MAX(ordinal) + 1 FROM yoink_runtime.delivery_units WHERE operation_id = l.id AND attempt = l.attempt), 0)
      AND l.delivered_artifact_count = COALESCE((SELECT SUM(artifact_count) FROM yoink_runtime.delivery_units WHERE operation_id=l.id AND attempt=l.attempt AND status='sent'), 0)
      AND $6 <= 10 AND $6 <= l.planned_artifact_count - l.delivered_artifact_count
      AND NOT EXISTS (SELECT 1 FROM yoink_runtime.delivery_units WHERE operation_id=l.id AND attempt=l.attempt AND status <> 'sent')
)
INSERT INTO yoink_runtime.delivery_units(operation_id, attempt, ordinal, artifact_count, status)
SELECT id, $2, $5, $6, 'sending' FROM checks
ON CONFLICT (operation_id, attempt, ordinal) DO NOTHING;