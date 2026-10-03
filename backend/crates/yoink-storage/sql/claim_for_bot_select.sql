SELECT id FROM runtime_operations
WHERE status='queued'
  AND available_at <= CAST(unixepoch('subsec') * 1000000 AS INTEGER)
  AND cancel_requested=0 AND attempt < max_attempts
  AND CAST(json_extract(request,'$.target.bot_id') AS TEXT)=?1
  AND (SELECT count(*) FROM runtime_operations AS active
       WHERE active.actor_id=runtime_operations.actor_id
         AND active.status IN ('running','delivering')) < ?2
ORDER BY available_at, created_at, id LIMIT 1;
