CREATE TABLE IF NOT EXISTS runtime_migrations (
    version INTEGER PRIMARY KEY NOT NULL,
    checksum TEXT NOT NULL,
    schema_hash TEXT NOT NULL,
    success INTEGER NOT NULL CHECK (success IN (0, 1)),
    installed_at INTEGER NOT NULL
);
