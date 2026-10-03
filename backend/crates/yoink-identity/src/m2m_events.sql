SELECT id, plugin, event_type, user_id, group_id, payload, created_at
FROM events
WHERE (?1 IS NULL OR plugin = ?1)
  AND (?2 IS NULL OR event_type = ?2)
ORDER BY created_at DESC
LIMIT ?3 OFFSET ?4;
