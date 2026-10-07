CREATE TABLE IF NOT EXISTS cookie_web_tokens (
 token_hash TEXT PRIMARY KEY CHECK(length(token_hash)=64),
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 expires_at INTEGER NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS cookie_web_tokens_actor ON cookie_web_tokens(user_id);
CREATE INDEX IF NOT EXISTS cookie_web_tokens_expiry ON cookie_web_tokens(expires_at);
CREATE TABLE IF NOT EXISTS cookie_oauth_sessions (
 session_hash TEXT PRIMARY KEY CHECK(length(session_hash)=64),
 user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 device_code TEXT NOT NULL CHECK(length(device_code)<=4096),
 ready_content TEXT CHECK(ready_content IS NULL OR length(CAST(ready_content AS BLOB))<=65536),
 expires_at INTEGER NOT NULL,
 poll_interval INTEGER NOT NULL,
 next_poll_at INTEGER NOT NULL,
 generation INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS cookie_oauth_sessions_actor ON cookie_oauth_sessions(user_id);
CREATE INDEX IF NOT EXISTS cookie_oauth_sessions_expiry ON cookie_oauth_sessions(expires_at);
