-- All timestamps are UTC epoch microseconds; ids break equal-count ties deterministically.
INSERT INTO users (id, username, first_name, role, language, created_at, updated_at, ban_until) VALUES
 (4001, 'owner', 'Owner', 'user', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), NULL),
 (4002, 'alpha_user', 'Alpha', 'user', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), NULL),
 (4003, 'beta_user', 'Beta', 'user', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), NULL),
 (4004, 'restricted_user', 'Restricted', 'restricted', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), NULL),
 (4005, 'banned_user', 'Banned', 'banned', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), NULL),
 (4006, 'timed_user', 'Timed Ban', 'user', 'en', CAST(unixepoch('subsec')*1000000 AS INTEGER), CAST(unixepoch('subsec')*1000000 AS INTEGER), (CAST(unixepoch('subsec')*1000000 AS INTEGER)+3600000000));
INSERT INTO download_log (user_id, url, domain, status, created_at) VALUES
 (4002, 'https://example.test/a', 'example.test', 'ok', CAST(unixepoch('subsec')*1000000 AS INTEGER)-86400000000),
 (4002, 'https://example.test/b', 'example.test', 'ok', CAST(unixepoch('subsec')*1000000 AS INTEGER)-864000000000),
 (4003, 'https://example.test/c', 'example.test', 'ok', CAST(unixepoch('subsec')*1000000 AS INTEGER)-691200000000),
 (4001, 'https://example.test/d', 'example.test', 'ok', CAST(unixepoch('subsec')*1000000 AS INTEGER)-86400000000);
