-- Module-owned proposal. Coordinator assigns identity/checksum, no global number reserved.
CREATE TABLE inbox_gh_mutations (
 id TEXT PRIMARY KEY NOT NULL,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 request_key TEXT CHECK(request_key IS NULL OR length(request_key) BETWEEN 1 AND 128),
 request TEXT NOT NULL CHECK(json_valid(request) AND json_type(request)='object'),
 state TEXT NOT NULL CHECK(state IN ('prepared','sending','unknown','confirmed','applied','rejected')),
 prepared TEXT CHECK(prepared IS NULL OR (json_valid(prepared) AND json_type(prepared)='object')),
 result TEXT CHECK(result IS NULL OR json_valid(result)),
 send_token TEXT,
 local_folder_id INTEGER REFERENCES inbox_gh_folders(id) ON DELETE SET NULL,
 error_code TEXT,
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 UNIQUE(user_id,request_key),
 CHECK(state NOT IN ('confirmed','applied') OR result IS NOT NULL),
 CHECK ((state='prepared' AND send_token IS NULL) OR (state<>'prepared' AND send_token IS NOT NULL))
) STRICT;
-- Serializes writes per user, including unknown outcomes, across process crashes.
-- Read-only recovery may run concurrently; send CAS admits exactly one caller.
CREATE UNIQUE INDEX inbox_gh_mutations_active ON inbox_gh_mutations(user_id)
 WHERE state IN ('prepared','sending','unknown','confirmed');
CREATE INDEX inbox_gh_mutations_recovery ON inbox_gh_mutations(state,updated_at,id);
