INSERT INTO users (id, username, role, language, theme, ban_until, created_at, updated_at) VALUES (91, 'preserved', 'banned', 'ru', 'frappe', 4102444800000000, 10, 20);
INSERT INTO groups (id, title, enabled, auto_grant_role, allow_pm, nsfw_allowed, created_at) VALUES (-91, 'disabled', 0, 'restricted', 0, 1, 10);
INSERT INTO user_group_policies (group_id, user_id, role_override, allow_pm_override) VALUES (-91, 91, 'moderator', 0);
INSERT INTO user_permissions (user_id, plugin, feature, granted_by, grant_source) VALUES (91, 'insight', 'tldr', 91, 'tag');
