-- Turso 0.7.2 parent-side FK probes must not read child rowid aliases as
-- ordinary record columns. Explicit indexes select the correct lookup path.
CREATE INDEX dl_user_settings_fk_user_id ON dl_user_settings(user_id);
CREATE INDEX inbox_gh_sync_state_fk_user_id ON inbox_gh_sync_state(user_id);
CREATE INDEX inbox_user_settings_fk_user_id ON inbox_user_settings(user_id);
CREATE INDEX insight_access_fk_user_id ON insight_access(user_id);
CREATE INDEX insight_user_byok_fk_user_id ON insight_user_byok(user_id);
CREATE INDEX insight_user_settings_fk_user_id ON insight_user_settings(user_id);
