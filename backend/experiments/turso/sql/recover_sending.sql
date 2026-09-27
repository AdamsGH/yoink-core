UPDATE jobs
SET state = 'delivery_unknown', lease_token = NULL, lease_expires_at = NULL
WHERE state = 'sending';
