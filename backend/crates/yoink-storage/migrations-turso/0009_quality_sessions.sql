CREATE TABLE runtime_quality_sessions (
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id > 0 AND chat_id = actor_id),
 id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision > 0),
 stage TEXT NOT NULL CHECK(stage IN ('choosing','confirming','submitted','cancelled')),
 origin_message_id INTEGER NOT NULL CHECK(origin_message_id BETWEEN 1 AND 2147483647),
 last_message_id INTEGER NOT NULL CHECK(last_message_id >= origin_message_id AND last_message_id <= 2147483647),
 request_json TEXT NOT NULL,
 selected TEXT,
 operation_id TEXT,
 PRIMARY KEY(actor_id, bot_id, chat_id),
 CHECK((stage IN ('confirming','submitted') AND selected IS NOT NULL) OR
       (stage IN ('choosing','cancelled') AND selected IS NULL)),
 CHECK((stage = 'submitted') = (operation_id IS NOT NULL)),
 CHECK(selected IS NULL OR selected IN ('best','2160','1440','1080','720','480','360','240','144')),
 CHECK(length(id) = 36),
 CHECK((stage = 'choosing' AND revision = 1) OR
       (stage IN ('confirming','cancelled') AND revision = 2) OR
       (stage = 'submitted' AND revision = 3))
) STRICT;
