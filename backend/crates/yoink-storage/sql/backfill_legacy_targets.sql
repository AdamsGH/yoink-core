WITH parsed AS (
    SELECT id, actor_id, regexp_match(idempotency_key, '^tg:([1-9][0-9]{0,18}):([1-9][0-9]{0,18}):([1-9][0-9]{0,9})$') AS parts
    FROM yoink_runtime.operations
), valid AS (
    SELECT id, actor_id, parts, parts[1]::numeric AS bot_num, parts[2]::numeric AS chat_num, parts[3]::numeric AS message_num
    FROM parsed
    WHERE parts IS NOT NULL AND parts[1]::numeric <= 9223372036854775807
      AND parts[2]::numeric = actor_id::numeric AND parts[3]::numeric <= 2147483647
)
UPDATE yoink_runtime.operations AS op SET request = op.request || jsonb_build_object(
    'target', jsonb_build_object(
        'bot_id', valid.bot_num::bigint, 'chat_id', valid.chat_num::bigint,
        'thread_id', NULL, 'reply_to_message_id', valid.message_num::integer
    ))
FROM valid WHERE op.id=valid.id AND (NOT (op.request ? 'target') OR op.request->'target'='null'::jsonb);