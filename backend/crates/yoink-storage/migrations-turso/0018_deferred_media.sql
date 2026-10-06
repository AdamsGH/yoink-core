-- Private-chat watermark survives consumption and restart. NULL means consumed.
CREATE TABLE runtime_deferred_media (
 actor_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id > 0 AND chat_id = actor_id),
 message_id INTEGER NOT NULL CHECK(message_id > 0),
 mode TEXT CHECK(mode IN ('video','audio','image')),
 PRIMARY KEY(actor_id, bot_id, chat_id)
);
-- Original Telegram intent is separate from the immutable resolved worker request.
CREATE TABLE runtime_telegram_media_intents (
 operation_id TEXT PRIMARY KEY NOT NULL REFERENCES runtime_operations(id) ON DELETE CASCADE,
 intent TEXT NOT NULL CHECK(json_valid(intent))
);
-- Owner quality drafts retain original Telegram intent across restart as well.
CREATE TABLE runtime_quality_media_intents (
 session_id TEXT PRIMARY KEY NOT NULL CHECK(length(session_id)=36),
 intent TEXT NOT NULL CHECK(json_valid(intent))
);
