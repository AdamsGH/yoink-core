-- All timestamps are UTC epoch microseconds.
INSERT INTO users (id, username, first_name, role, language, created_at, updated_at, ban_until) VALUES
 (1001, 'configured', 'Owner', 'user', 'en', 1700000000000000, 1700000000000000, NULL),
 (1002, 'admin', 'Administrator', 'admin', 'en', 1700000000000000, 1700000000000000, NULL),
 (1003, 'target', 'Target', 'user', 'en', 1700000000000000, 1700000000000000, NULL),
 (1004, 'promotable', 'Promotable', 'user', 'en', 1700000000000000, 1700000000000000, NULL),
 (1005, 'timed', 'Timed', 'user', 'en', 1700000000000000, 1700000000000000, 4102444800123456);
