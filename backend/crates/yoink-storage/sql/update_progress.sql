UPDATE yoink_runtime.operations
SET progress_percent = $5, progress_stage = $6, updated_at = clock_timestamp()
WHERE id = $1 AND attempt = $2 AND lease_token = $3 AND worker_id = $4
  AND status = 'running' AND lease_expires_at > clock_timestamp() AND NOT cancel_requested;
