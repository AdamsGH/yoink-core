UPDATE yoink_runtime.operations
SET status = CASE WHEN $5 AND attempt < max_attempts AND NOT cancel_requested THEN 'queued' ELSE 'failed' END,
    worker_id = NULL, lease_token = NULL, lease_expires_at = NULL,
    updated_at = clock_timestamp()
WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4
  AND status = 'running' AND lease_expires_at > clock_timestamp();
