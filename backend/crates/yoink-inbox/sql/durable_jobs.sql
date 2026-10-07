-- Coordinator assigns the migration identity; run after native application schema.
CREATE TABLE inbox_jobs (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 item_id INTEGER REFERENCES inbox_items(id) ON DELETE CASCADE,
 kind TEXT NOT NULL CHECK(kind IN ('enrich','classify','stars_sync')),
 state TEXT NOT NULL DEFAULT 'ready' CHECK(state IN ('ready','running','done','failed')),
 attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
 available_at INTEGER NOT NULL,
 lease_until INTEGER,
 lease_token TEXT,
 last_error TEXT,
 created_at INTEGER NOT NULL,
 CHECK ((kind='stars_sync' AND item_id IS NULL) OR (kind<>'stars_sync' AND item_id IS NOT NULL)),
 CHECK ((state='running' AND lease_until IS NOT NULL AND lease_token IS NOT NULL) OR (state<>'running' AND lease_until IS NULL AND lease_token IS NULL))
) STRICT;
CREATE UNIQUE INDEX inbox_jobs_active ON inbox_jobs(user_id,kind,coalesce(item_id,0)) WHERE state IN ('ready','running');
CREATE INDEX inbox_jobs_ready ON inbox_jobs(state,available_at,id);
