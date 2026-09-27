WITH candidate AS (
    SELECT id
    FROM yoink_runtime.operations
    WHERE status = 'queued' AND NOT cancel_requested AND attempt < max_attempts
    ORDER BY created_at, id
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
UPDATE yoink_runtime.operations AS op
SET status = 'running',
    attempt = op.attempt + 1,
    worker_id = $1,
    lease_token = gen_random_uuid(),
    lease_expires_at = clock_timestamp() + ($2::bigint * INTERVAL '1 second'),
    updated_at = clock_timestamp()
FROM candidate
WHERE op.id = candidate.id
RETURNING op.id, op.attempt, op.lease_token, op.worker_id, op.request,
          op.cancel_requested, op.lease_expires_at;
