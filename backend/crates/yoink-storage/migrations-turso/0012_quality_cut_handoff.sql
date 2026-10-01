-- Preserve the complete quality row while adding a durable handoff state.
CREATE TABLE runtime_quality_sessions_v11 AS SELECT * FROM runtime_quality_sessions;
DROP TABLE runtime_quality_sessions;
CREATE TABLE runtime_quality_sessions (
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id > 0 AND chat_id = actor_id),
 id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision > 0),
 stage TEXT NOT NULL CHECK(stage IN ('choosing','confirming','submitted','cancelled','cutting')),
 origin_message_id INTEGER NOT NULL CHECK(origin_message_id BETWEEN 1 AND 2147483647),
 last_message_id INTEGER NOT NULL CHECK(last_message_id >= origin_message_id AND last_message_id <= 2147483647),
 request_json TEXT NOT NULL,
 selected TEXT,
 operation_id TEXT,
 linked_cut_id TEXT,
 PRIMARY KEY(actor_id, bot_id, chat_id),
 CHECK((stage IN ('confirming','submitted') AND selected IS NOT NULL) OR
       (stage IN ('choosing','cancelled','cutting') AND selected IS NULL)),
 CHECK((stage = 'submitted') = (operation_id IS NOT NULL)),
 CHECK(selected IS NULL OR selected IN ('best','audio','2160','1440','1080','720','480','360','240','144')),
 CHECK(length(id) = 36),
 CHECK((stage = 'choosing' AND revision = 1) OR
       (stage IN ('confirming','cancelled','cutting') AND revision >= 2) OR
       (stage = 'submitted' AND revision = 3)),
 CHECK((stage = 'cutting') = (linked_cut_id IS NOT NULL)),
 CHECK(linked_cut_id IS NULL OR length(linked_cut_id) = 36)
) STRICT;
INSERT INTO runtime_quality_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,request_json,selected,operation_id,linked_cut_id)
SELECT actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,request_json,selected,operation_id,NULL FROM runtime_quality_sessions_v11;
DROP TABLE runtime_quality_sessions_v11;
-- ADD COLUMN also changes the pinned engine's DDL formatting during VACUUM INTO.
-- Rebuild without ALTER to keep the schema fingerprint stable in snapshots.
CREATE TABLE runtime_cut_sessions_v11 AS SELECT * FROM runtime_cut_sessions;
DROP TABLE runtime_cut_sessions;
CREATE TABLE runtime_cut_sessions (
 actor_id INTEGER NOT NULL CHECK(actor_id > 0),
 bot_id INTEGER NOT NULL CHECK(bot_id > 0),
 chat_id INTEGER NOT NULL CHECK(chat_id > 0 AND chat_id = actor_id),
 id TEXT NOT NULL,
 revision INTEGER NOT NULL CHECK(revision > 0),
 stage TEXT NOT NULL CHECK(stage IN ('start','end','ready','quality','confirming','submitted','cancelled')),
 origin_message_id INTEGER NOT NULL CHECK(origin_message_id BETWEEN 1 AND 2147483647),
 last_message_id INTEGER NOT NULL CHECK(last_message_id BETWEEN origin_message_id AND 2147483647),
 source_url TEXT,
 begin_start REAL,
 start REAL,
 end REAL,
 operation_id TEXT,
 selected TEXT,
 original_request_json TEXT CHECK(original_request_json IS NULL OR length(original_request_json) > 1),
 linked_quality_id TEXT CHECK(linked_quality_id IS NULL OR length(linked_quality_id) = 36),
 PRIMARY KEY(actor_id, bot_id, chat_id),
 CHECK((stage = 'cancelled') OR source_url IS NOT NULL),
 CHECK(begin_start IS NULL OR begin_start >= 0),
 CHECK(start IS NULL OR start >= 0),
 CHECK((stage NOT IN ('ready','quality','confirming','submitted')) OR (start IS NOT NULL AND end IS NOT NULL AND end > start)),
 CHECK((stage = 'quality' AND selected IS NULL) OR (stage != 'quality' AND (selected IS NULL OR stage IN ('confirming','submitted')))),
 CHECK(selected IS NULL OR selected IN ('best','2160','1440','1080','720','480','360','240','144')),
 CHECK((stage = 'submitted') = (operation_id IS NOT NULL))
) STRICT;
INSERT INTO runtime_cut_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected,original_request_json,linked_quality_id)
SELECT actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected,NULL,NULL FROM runtime_cut_sessions_v11;
DROP TABLE runtime_cut_sessions_v11;
