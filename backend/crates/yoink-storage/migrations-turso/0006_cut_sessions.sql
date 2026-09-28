-- One durable private cut draft per actor, bot, and chat.
CREATE TABLE runtime_cut_sessions (
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id > 0 AND chat_id = actor_id),
 id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision > 0),
 stage TEXT NOT NULL CHECK(stage IN ('start','end','ready','confirming','submitted','cancelled')),
 origin_message_id INTEGER NOT NULL CHECK(origin_message_id BETWEEN 1 AND 2147483647),
 last_message_id INTEGER NOT NULL CHECK(last_message_id BETWEEN origin_message_id AND 2147483647),
 source_url TEXT,
 begin_start REAL,
 start REAL,
 end REAL,
 operation_id TEXT,
 PRIMARY KEY(actor_id, bot_id, chat_id),
 CHECK((stage = 'cancelled') OR source_url IS NOT NULL),
 CHECK(begin_start IS NULL OR begin_start >= 0),
 CHECK((stage NOT IN ('ready','confirming','submitted')) OR (start IS NOT NULL AND end IS NOT NULL AND end > start)),
 CHECK((stage = 'submitted') = (operation_id IS NOT NULL))
) STRICT;
