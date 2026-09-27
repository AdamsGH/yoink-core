UPDATE jobs SET state = 'sending'
WHERE id = ?1 AND state = 'leased' AND lease_generation = ?2 AND lease_token = ?3
  AND lease_expires_at > ?4;
