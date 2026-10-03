-- Rebuild the cut table rather than ALTER: pinned Turso rewrites unrelated trigger DDL on VACUUM INTO.
CREATE TABLE runtime_cut_sessions_v12 AS SELECT * FROM runtime_cut_sessions;
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
 clips_json TEXT NOT NULL CHECK(length(clips_json) >= 2),
 editing_index INTEGER CHECK(editing_index IS NULL OR editing_index BETWEEN 0 AND 19),
 PRIMARY KEY(actor_id, bot_id, chat_id),
 CHECK((stage = 'cancelled') OR source_url IS NOT NULL),
 CHECK(begin_start IS NULL OR begin_start >= 0),
 CHECK(start IS NULL OR start >= 0),
 CHECK((stage NOT IN ('ready','quality','confirming','submitted')) OR (start IS NOT NULL AND end IS NOT NULL AND end > start)),
 CHECK((stage = 'quality' AND selected IS NULL) OR (stage != 'quality' AND (selected IS NULL OR stage IN ('confirming','submitted')))),
 CHECK(selected IS NULL OR selected IN ('best','2160','1440','1080','720','480','360','240','144')),
 CHECK((stage = 'submitted') = (operation_id IS NOT NULL))
) STRICT;
INSERT INTO runtime_cut_sessions(actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected,original_request_json,linked_quality_id,clips_json,editing_index)
SELECT actor_id,bot_id,chat_id,id,revision,stage,origin_message_id,last_message_id,source_url,begin_start,start,end,operation_id,selected,original_request_json,linked_quality_id,
 CASE WHEN stage IN ('ready','quality','confirming','submitted') AND start IS NOT NULL AND end IS NOT NULL AND end > start
      THEN '[[' || start || ',' || end || ']]' ELSE '[]' END,
 NULL
FROM runtime_cut_sessions_v12;
DROP TABLE runtime_cut_sessions_v12;
