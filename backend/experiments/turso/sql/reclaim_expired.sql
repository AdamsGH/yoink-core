UPDATE jobs
SET state = 'queued', lease_token = NULL, lease_expires_at = NULL
WHERE state = 'leased' AND lease_expires_at <= ?1;
