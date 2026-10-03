INSERT INTO users (id, username, first_name, role, language, theme, ban_until, created_at, updated_at) VALUES
(1, 'owner', 'Owner', 'user', 'en', 'mocha', NULL, 10, 20),
(2, 'admin', 'Admin', 'admin', 'en', 'mocha', NULL, 10, 20),
(3, 'original', 'Original', 'restricted', 'ru', 'frappe', NULL, 10, 20),
(4, 'banned', 'Banned', 'banned', 'ru', 'frappe', 4102444800000000, 10, 20),
(5, 'ordinary', 'Ordinary', 'user', 'en', 'mocha', NULL, 10, 20),
(6, 'imported', 'Imported', 'user', 'en', 'mocha', NULL, 10, 20);
INSERT INTO user_permissions (user_id, plugin, feature, granted_by, grant_source) VALUES (6, 'insight', 'tldr', 1, 'tag');
INSERT INTO bot_settings (key, value, updated_at) VALUES ('tag_map', '{"VIP":["insight:tldr"],"other":["insight:summary"]}', 10);
