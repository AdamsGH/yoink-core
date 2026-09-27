UPDATE yoink_runtime.operations
SET lease_expires_at = clock_timestamp() + ($5::bigint * INTERVAL '1 second'),
    updated_at = clock_timestamp()
WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4
  AND status = 'running' AND lease_expires_at > clock_timestamp()
RETURNING lease_expires_at;
