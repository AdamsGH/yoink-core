SELECT (SELECT COUNT(*) FROM users), (SELECT COUNT(*) FROM groups),
       (SELECT COUNT(*) FROM groups WHERE enabled = 1);
