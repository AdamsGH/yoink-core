SELECT count(*) FROM runtime_operations WHERE actor_id = 10009 AND idempotency_key = 'rolled-back';
