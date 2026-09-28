INSERT INTO runtime_operations (
    id, actor_id, idempotency_key, request, status, attempt, max_attempts,
    cancel_requested, created_at, updated_at, available_at, retry_delay_secs,
    delivered_artifact_count, cache_bypassed
)
VALUES
    ('00000000-0000-0000-0000-000000023101', 23101, 'tg:71:23101:9', '{"actor_id":23101,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000001, 1700000000000001, 1700000000000001, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023102', 23102, 'tg:071:23102:9', '{"actor_id":23102,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000002, 1700000000000002, 1700000000000002, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023103', 23103, 'tg:9223372036854775808:23103:9', '{"actor_id":23103,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000003, 1700000000000003, 1700000000000003, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023104', 23104, 'tg:71:9223372036854775808:9', '{"actor_id":23104,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000004, 1700000000000004, 1700000000000004, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023105', 23105, 'tg:71:23105:2147483648', '{"actor_id":23105,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000005, 1700000000000005, 1700000000000005, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023106', 23106, 'tg:71:99999:9', '{"actor_id":23106,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}', 'queued', 0, 1, 0, 1700000000000006, 1700000000000006, 1700000000000006, 0, 0, 0),
    ('00000000-0000-0000-0000-000000023107', 23107, 'tg:77:23107:9', '{"actor_id":23107,"source_url":"https://example.com/existing","media_mode":"video","clip_range":null,"max_height":null,"target":{"bot_id":88,"chat_id":23107,"thread_id":null,"reply_to_message_id":17}}', 'queued', 0, 1, 0, 1700000000000007, 1700000000000007, 1700000000000007, 0, 0, 0);
