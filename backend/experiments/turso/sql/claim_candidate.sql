SELECT id, lease_generation
FROM jobs
WHERE state = 'queued'
ORDER BY id
LIMIT 1;
