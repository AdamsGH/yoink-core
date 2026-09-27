SELECT ordinal, status, artifact_count, message_ids
FROM yoink_runtime.delivery_units
WHERE operation_id = $1 AND attempt = $2
ORDER BY ordinal;
