-- Use the preserved rate_limits PK (user_id,window) and UTC microsecond reset_at
-- directly: legacy minute/hour/day anchored counters are compatible. Unknown
-- window names are preserved but are not downloader windows. No reverse migration.
-- The active safety count spans bots/chats/topics and includes delivery/cache sends.
CREATE INDEX operations_active_actor ON runtime_operations(actor_id) WHERE status IN ('running','delivering');
