SELECT 1 AS allowed FROM users WHERE id=?1
AND (role IN ('user','moderator','admin','owner') OR (?3=1 AND role='restricted'))
AND (ban_until IS NULL OR ban_until<=?2);
