WITH candidate AS (
    SELECT id FROM yoink_runtime.operations
    WHERE status = 'queued' AND available_at <= clock_timestamp() AND NOT cancel_requested AND attempt < max_attempts
      AND request->'target'->>'bot_id' = $3
    ORDER BY available_at, created_at, id FOR UPDATE SKIP LOCKED LIMIT 1
)
UPDATE yoink_runtime.operations AS op
SET status = 'running', attempt = op.attempt + 1, worker_id = $1, lease_token = gen_random_uuid(),
    lease_expires_at = clock_timestamp() + ($2::bigint * INTERVAL '1 second'), updated_at = clock_timestamp(),
    progress_percent = NULL, progress_stage = NULL, planned_artifact_count = NULL, delivered_artifact_count = 0
FROM candidate WHERE op.id = candidate.id
RETURNING op.id, op.attempt, op.lease_token, op.worker_id, op.request, op.cancel_requested, op.lease_expires_at;