-- Native fixture timestamps use UTC epoch microseconds.
INSERT INTO users
    (id, role, language, theme, is_premium, created_at, updated_at, ban_until)
VALUES
    (8000000000011, 'user', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL),
    (8000000000012, 'restricted', 'ru', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL),
    (8000000000013, 'banned', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL),
    (8000000000014, 'user', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, (CAST(strftime('%s', 'now') AS INTEGER) + 3600) * 1000000),
    (8000000000015, 'user', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL),
    (8000000000016, 'user', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL),
    (8000000000017, 'user', 'en', 'macchiato', 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, (CAST(strftime('%s', 'now') AS INTEGER) - 3600) * 1000000);

INSERT INTO user_permissions
    (user_id, plugin, feature, granted_by, granted_at, grant_source, expires_at)
VALUES
    (8000000000011, 'dl', 'inline', 8000000000011, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, 'manual', NULL),
    (8000000000012, 'dl', 'inline', 8000000000011, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, 'manual', (CAST(strftime('%s', 'now') AS INTEGER) + 3600) * 1000000),
    (8000000000013, 'dl', 'inline', 8000000000011, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, 'manual', NULL),
    (8000000000015, 'dl', 'inline', 8000000000011, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, 'manual', CAST(strftime('%s', 'now') AS INTEGER) * 1000000),
    (8000000000016, 'dl', 'inline', 8000000000011, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, 'manual', (CAST(strftime('%s', 'now') AS INTEGER) - 1) * 1000000);

INSERT INTO insight_user_byok
    (user_id, provider, base_url, api_key, model, models_json, models_fetched_at,
     tested_at, test_error, created_at, updated_at)
VALUES
    (8000000000011, 'openai', 'https://fake.example', 'integration-fake-key', 'fake-model', NULL, NULL,
     CAST(strftime('%s', 'now') AS INTEGER) * 1000000, NULL, CAST(strftime('%s', 'now') AS INTEGER) * 1000000, CAST(strftime('%s', 'now') AS INTEGER) * 1000000);

INSERT INTO groups
    (id, enabled, auto_grant_role, allow_pm, nsfw_allowed, created_at)
VALUES
    (-8000000000011, 1, 'moderator', 1, 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000),
    (-8000000000012, 0, 'admin', 1, 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000000);

INSERT INTO user_group_policies
    (user_id, group_id, role_override, allow_pm_override)
VALUES
    (8000000000012, -8000000000011, 'admin', NULL);

INSERT INTO thread_policies (group_id, thread_id, name, enabled) VALUES
    (-8000000000011, NULL, NULL, 1),
    (-8000000000011, 17, NULL, 0),
    (-8000000000011, 18, NULL, 1);
