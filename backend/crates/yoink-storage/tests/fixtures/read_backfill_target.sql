SELECT json_extract(request, '$.target') FROM runtime_operations WHERE ?1 = idempotency_key;
