UPDATE runtime_operations
SET lease_expires_at = CAST(unixepoch('subsec') * 1000000 AS INTEGER) - 1000000
WHERE id = ?1;
