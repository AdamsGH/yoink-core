-- Canonical Telegram ordering. Cross-bot date ties are first-accepted-wins.
CREATE TABLE stats_ingestion_watermarks (
 bot_id INTEGER NOT NULL, kind TEXT NOT NULL, chat_id INTEGER NOT NULL,
 subject_id INTEGER NOT NULL, actor_id INTEGER NOT NULL, event_date INTEGER NOT NULL,
 update_id INTEGER NOT NULL, PRIMARY KEY(kind,chat_id,subject_id,actor_id)
) STRICT;
CREATE TABLE stats_runtime_revision (id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL) STRICT;
INSERT INTO stats_runtime_revision VALUES(1,0);
CREATE TABLE stats_subject_revisions(kind TEXT NOT NULL,chat_id INTEGER NOT NULL,subject_id INTEGER NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(kind,chat_id,subject_id)) STRICT;
CREATE TABLE stats_refresh_fences(kind TEXT NOT NULL,chat_id INTEGER NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(kind,chat_id)) STRICT;
-- Imported event_floor is immutable; refresh_floor advances only after successful reconciliation.
CREATE TABLE stats_name_baselines(user_id INTEGER PRIMARY KEY,event_floor INTEGER NOT NULL,refresh_floor INTEGER) STRICT;
CREATE TABLE stats_roster_floors(chat_id INTEGER PRIMARY KEY,event_floor INTEGER NOT NULL) STRICT;
CREATE INDEX stats_native_chat_date_row ON stats_messages(chat_id,date,id);
CREATE TRIGGER stats_revision_watermark_insert AFTER INSERT ON stats_ingestion_watermarks BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT NEW.kind,NEW.chat_id,NEW.subject_id,revision FROM stats_runtime_revision WHERE id=1 AND NEW.kind IN ('name','member') ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_watermark_update AFTER UPDATE ON stats_ingestion_watermarks BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT NEW.kind,NEW.chat_id,NEW.subject_id,revision FROM stats_runtime_revision WHERE id=1 AND NEW.kind IN ('name','member') ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_messages_insert AFTER INSERT ON stats_messages BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_messages_update AFTER UPDATE ON stats_messages BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_messages_delete AFTER DELETE ON stats_messages BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_reactions_insert AFTER INSERT ON stats_reactions BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_reactions_update AFTER UPDATE ON stats_reactions BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_reactions_delete AFTER DELETE ON stats_reactions BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_user_names_insert AFTER INSERT ON stats_user_names BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'name',0,NEW.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_user_names_update AFTER UPDATE ON stats_user_names BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'name',0,NEW.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_user_names_delete AFTER DELETE ON stats_user_names BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'name',0,OLD.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_group_members_insert AFTER INSERT ON stats_group_members BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'member',NEW.chat_id,NEW.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_group_members_update AFTER UPDATE ON stats_group_members BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'member',NEW.chat_id,NEW.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_group_members_delete AFTER DELETE ON stats_group_members BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
 INSERT INTO stats_subject_revisions SELECT 'member',OLD.chat_id,OLD.user_id,revision FROM stats_runtime_revision WHERE id=1 ON CONFLICT(kind,chat_id,subject_id) DO UPDATE SET revision=excluded.revision;
END;
CREATE TRIGGER stats_revision_stats_chat_admins_insert AFTER INSERT ON stats_chat_admins BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_chat_admins_update AFTER UPDATE ON stats_chat_admins BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_chat_admins_delete AFTER DELETE ON stats_chat_admins BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_user_events_insert AFTER INSERT ON stats_user_events BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_user_events_update AFTER UPDATE ON stats_user_events BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_stats_user_events_delete AFTER DELETE ON stats_user_events BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_users_insert AFTER INSERT ON users BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_users_update AFTER UPDATE ON users BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_users_delete AFTER DELETE ON users BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_groups_insert AFTER INSERT ON groups BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_groups_update AFTER UPDATE ON groups BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_groups_delete AFTER DELETE ON groups BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_user_group_policies_insert AFTER INSERT ON user_group_policies BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_user_group_policies_update AFTER UPDATE ON user_group_policies BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
CREATE TRIGGER stats_revision_user_group_policies_delete AFTER DELETE ON user_group_policies BEGIN
 UPDATE stats_runtime_revision SET revision=revision+1 WHERE id=1;
END;
