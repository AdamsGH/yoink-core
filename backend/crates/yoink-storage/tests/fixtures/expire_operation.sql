UPDATE yoink_runtime.operations
SET lease_expires_at = clock_timestamp() - INTERVAL '1 second'
WHERE id = $1;
