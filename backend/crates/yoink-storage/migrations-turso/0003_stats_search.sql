CREATE TABLE stats_search_state (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    database_id BLOB NOT NULL CHECK (length(database_id) = 32),
    source_revision INTEGER NOT NULL CHECK (source_revision >= 0),
    acknowledged_revision INTEGER NOT NULL CHECK (acknowledged_revision >= 0 AND acknowledged_revision <= source_revision)
) STRICT;

CREATE TABLE stats_search_dirty (
    id INTEGER PRIMARY KEY NOT NULL
) STRICT;

INSERT INTO stats_search_state (singleton, database_id, source_revision, acknowledged_revision)
VALUES (1, randomblob(32), 0, 0);

CREATE TRIGGER stats_messages_search_insert AFTER INSERT ON stats_messages BEGIN
    INSERT INTO stats_search_dirty(id) VALUES (NEW.id) ON CONFLICT(id) DO NOTHING;
    UPDATE stats_search_state SET source_revision = source_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER stats_messages_search_update AFTER UPDATE ON stats_messages BEGIN
    INSERT INTO stats_search_dirty(id) VALUES (OLD.id) ON CONFLICT(id) DO NOTHING;
    INSERT INTO stats_search_dirty(id) VALUES (NEW.id) ON CONFLICT(id) DO NOTHING;
    UPDATE stats_search_state SET source_revision = source_revision + 1 WHERE singleton = 1;
END;

CREATE TRIGGER stats_messages_search_delete AFTER DELETE ON stats_messages BEGIN
    INSERT INTO stats_search_dirty(id) VALUES (OLD.id) ON CONFLICT(id) DO NOTHING;
    UPDATE stats_search_state SET source_revision = source_revision + 1 WHERE singleton = 1;
END;

INSERT INTO stats_search_dirty(id) SELECT id FROM stats_messages;
