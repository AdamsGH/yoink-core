-- Only confirmed Bot API message IDs are registered. Restart never sends a new notice.
CREATE TABLE runtime_progress_notices (
 operation_id TEXT PRIMARY KEY NOT NULL REFERENCES runtime_operations(id) ON DELETE CASCADE,
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id <> 0),
 message_id INTEGER NOT NULL CHECK(message_id BETWEEN 1 AND 2147483647),
 language TEXT NOT NULL CHECK(language IN ('en','ru')),
 last_text TEXT CHECK(last_text IS NULL OR length(CAST(last_text AS BLOB)) <= 4096),
 finished INTEGER NOT NULL DEFAULT 0 CHECK(finished IN (0,1)),
 next_attempt_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000000 AS INTEGER))
);
CREATE INDEX progress_notices_due ON runtime_progress_notices(bot_id,next_attempt_at,operation_id) WHERE finished=0;
