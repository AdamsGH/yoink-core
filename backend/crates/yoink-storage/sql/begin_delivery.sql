UPDATE yoink_runtime.operations SET status = 'delivering', delivery_started_at = clock_timestamp(), updated_at = clock_timestamp()
WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4 AND status = 'running'
  AND NOT cancel_requested AND lease_expires_at > clock_timestamp();