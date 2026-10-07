-- Module-owned migration proposal; coordinator assigns global identity/checksum.
CREATE TABLE inbox_notifications (
 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
 id TEXT NOT NULL UNIQUE,
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 item_id INTEGER REFERENCES inbox_items(id) ON DELETE SET NULL,
 completion_key TEXT NOT NULL UNIQUE,
 payload TEXT NOT NULL CHECK(json_valid(payload) AND json_type(payload)='object'),
 chat_id INTEGER,
 message_id INTEGER,
 state TEXT NOT NULL CHECK(state IN ('waiting','ready','claimed','sending','unknown','acked','cancelled')),
 attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
 available_at INTEGER NOT NULL,
 lease_until INTEGER,
 lease_token TEXT,
 send_token TEXT,
 receipt TEXT,
 error_code TEXT CHECK(error_code IS NULL OR error_code IN ('not_sent','outcome_unknown','inactive')),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 CHECK((chat_id IS NULL AND message_id IS NULL) OR (chat_id<>0 AND message_id>0)),
 CHECK(state NOT IN ('ready','claimed','sending','unknown','acked') OR (chat_id IS NOT NULL AND message_id IS NOT NULL)),
 CHECK(state NOT IN ('sending','unknown','acked') OR send_token IS NOT NULL),
 CHECK(state<>'acked' OR receipt IS NOT NULL),
 CHECK(state<>'claimed' OR (lease_until IS NOT NULL AND lease_token IS NOT NULL))
) STRICT;
CREATE INDEX inbox_notifications_poll ON inbox_notifications(state,available_at,sequence);
CREATE INDEX inbox_notifications_item ON inbox_notifications(user_id,item_id);
