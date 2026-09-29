SELECT 1 AS allowed
FROM users
WHERE id=?1 AND role<>'banned'
 AND (ban_until IS NULL OR ban_until<=CAST(unixepoch('subsec')*1000000 AS INTEGER));
