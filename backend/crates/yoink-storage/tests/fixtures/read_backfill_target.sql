SELECT request->'target' FROM yoink_runtime.operations WHERE idempotency_key = $1;
