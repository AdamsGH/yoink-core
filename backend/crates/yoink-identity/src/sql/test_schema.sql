CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    role TEXT NOT NULL,
    language TEXT NOT NULL,
    ban_until INTEGER
);
CREATE TABLE user_permissions (
    user_id INTEGER NOT NULL,
    plugin TEXT NOT NULL,
    feature TEXT NOT NULL,
    expires_at INTEGER
);
CREATE TABLE bot_settings (
    key TEXT PRIMARY KEY,
    value TEXT
);
CREATE TABLE insight_user_byok (
    user_id INTEGER PRIMARY KEY,
    tested_at INTEGER,
    test_error TEXT,
    api_key TEXT
);
CREATE TABLE groups (
    id INTEGER PRIMARY KEY,
    enabled INTEGER NOT NULL,
    auto_grant_role TEXT NOT NULL
);
CREATE TABLE user_group_policies (
    group_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    role_override TEXT,
    PRIMARY KEY (group_id, user_id)
);
CREATE TABLE thread_policies (
    group_id INTEGER NOT NULL,
    thread_id INTEGER,
    enabled INTEGER NOT NULL
);
