UPDATE jobs
SET state = 'leased',
    lease_generation = ?2,
    lease_token = ?3,
    lease_expires_at = ?4
WHERE id = ?1 AND state = 'queued';
