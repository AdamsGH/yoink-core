INSERT INTO runtime_cut_sessions
 (actor_id, bot_id, chat_id, id, revision, stage, origin_message_id,
  last_message_id, source_url, begin_start, start, end, operation_id, selected)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(actor_id, bot_id, chat_id) DO UPDATE SET
 id=excluded.id, revision=excluded.revision, stage=excluded.stage,
 origin_message_id=excluded.origin_message_id, last_message_id=excluded.last_message_id,
 source_url=excluded.source_url, begin_start=excluded.begin_start,
 start=excluded.start, end=excluded.end,
 operation_id=excluded.operation_id, selected=excluded.selected;
