SELECT state, lease_generation, lease_token, lease_expires_at
FROM jobs WHERE id = ?1;
