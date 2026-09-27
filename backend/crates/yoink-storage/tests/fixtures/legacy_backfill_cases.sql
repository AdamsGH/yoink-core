INSERT INTO yoink_runtime.operations (actor_id, idempotency_key, request, max_attempts)
VALUES
    (23101, 'tg:71:23101:9', '{"actor_id":23101,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23102, 'tg:071:23102:9', '{"actor_id":23102,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23103, 'tg:9223372036854775808:23103:9', '{"actor_id":23103,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23104, 'tg:71:9223372036854775808:9', '{"actor_id":23104,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23105, 'tg:71:23105:2147483648', '{"actor_id":23105,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23106, 'tg:71:99999:9', '{"actor_id":23106,"source_url":"https://example.com/backfill","media_mode":"video","clip_range":null,"max_height":null}'::jsonb, 1),
    (23107, 'tg:77:23107:9', '{"actor_id":23107,"source_url":"https://example.com/existing","media_mode":"video","clip_range":null,"max_height":null,"target":{"bot_id":88,"chat_id":23107,"thread_id":null,"reply_to_message_id":17}}'::jsonb, 1);
