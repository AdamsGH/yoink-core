UPDATE yoink_runtime.operations
SET status = 'failed', worker_id = NULL, lease_token = NULL, lease_expires_at = NULL, updated_at = clock_timestamp()
WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4 AND status = 'delivering'
  AND lease_expires_at > clock_timestamp();