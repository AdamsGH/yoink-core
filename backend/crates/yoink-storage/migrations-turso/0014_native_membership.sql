-- Observation and deletion state intentionally survive group deletion: they are
-- ordering fences and durable menu cleanup targets, not administrative policies.
CREATE TABLE membership_group_suppressions (
    group_id INTEGER PRIMARY KEY CHECK (group_id < 0)
) STRICT;
CREATE TABLE membership_bots (
    source_id INTEGER NOT NULL CHECK (source_id > 0),
    group_id INTEGER NOT NULL CHECK (group_id < 0),
    occurred_at INTEGER NOT NULL,
    update_id INTEGER NOT NULL CHECK (update_id BETWEEN 0 AND 4294967295),
    payload TEXT NOT NULL,
    present INTEGER NOT NULL CHECK (present IN (0, 1)),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    PRIMARY KEY (source_id, group_id)
) STRICT;
CREATE INDEX membership_bots_group ON membership_bots (group_id, present);
CREATE TABLE membership_users (
    source_id INTEGER NOT NULL CHECK (source_id > 0),
    group_id INTEGER NOT NULL CHECK (group_id < 0),
    user_id INTEGER NOT NULL CHECK (user_id > 0),
    occurred_at INTEGER NOT NULL,
    update_id INTEGER NOT NULL CHECK (update_id BETWEEN 0 AND 4294967295),
    payload TEXT NOT NULL,
    present INTEGER NOT NULL CHECK (present IN (0, 1)),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    tag TEXT,
    tag_generation INTEGER NOT NULL CHECK (tag_generation >= 0),
    PRIMARY KEY (source_id, group_id, user_id)
) STRICT;
CREATE INDEX membership_users_user ON membership_users (user_id, group_id);
CREATE INDEX membership_users_group ON membership_users (group_id, source_id);
CREATE TABLE membership_feature_suppressions (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plugin TEXT NOT NULL,
    feature TEXT NOT NULL,
    PRIMARY KEY (user_id, plugin, feature)
) STRICT;
