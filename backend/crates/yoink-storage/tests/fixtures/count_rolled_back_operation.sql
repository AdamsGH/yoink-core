SELECT count(*) FROM yoink_runtime.operations WHERE actor_id = 10009 AND idempotency_key = 'rolled-back';
