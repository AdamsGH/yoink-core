SELECT id, plugin, event_type, user_id, group_id, payload, created_at
FROM events
WHERE id = last_insert_rowid();
