UPDATE jobs
SET state = 'sent', lease_token = NULL, lease_expires_at = NULL
WHERE id = ?1 AND state = 'leased'
  AND lease_generation = ?2 AND lease_token = ?3 AND lease_expires_at > ?4;
