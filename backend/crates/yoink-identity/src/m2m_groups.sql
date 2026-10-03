SELECT id, title, enabled, auto_grant_role, created_at
FROM groups
WHERE (?1 = 0 OR enabled = 1)
ORDER BY id;
