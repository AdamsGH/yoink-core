UPDATE runtime_quality_sessions SET stage = 'confirming', revision = 2, operation_id = NULL WHERE actor_id = ?1 AND selected = 'audio';
