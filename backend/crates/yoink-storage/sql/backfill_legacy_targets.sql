WITH raw AS (
 SELECT id,actor_id,substr(idempotency_key,4) AS tail
 FROM runtime_operations
 WHERE substr(idempotency_key,1,3)='tg:'
), first_part AS (
 SELECT id,actor_id,tail,instr(tail,':') AS c1
 FROM raw
), second_part AS (
 SELECT id,actor_id,tail,c1,substr(tail,1,c1-1) AS bot,substr(tail,c1+1) AS rest
 FROM first_part WHERE c1>1
), parts AS (
 SELECT id,actor_id,bot,rest,instr(rest,':') AS c2
 FROM second_part
), valid AS (
 SELECT id,actor_id,bot,substr(rest,1,c2-1) AS chat,substr(rest,c2+1) AS message
 FROM parts WHERE c2>1 AND instr(substr(rest,c2+1),':')=0
)
UPDATE runtime_operations
SET request=json_set(request,'$.target',json_object(
 'bot_id',CAST(valid.bot AS INTEGER),'chat_id',CAST(valid.chat AS INTEGER),
 'thread_id',NULL,'reply_to_message_id',CAST(valid.message AS INTEGER)))
FROM valid
WHERE runtime_operations.id=valid.id
 AND (json_type(runtime_operations.request,'$.target') IS NULL OR json_type(runtime_operations.request,'$.target')='null')
 AND length(valid.bot) BETWEEN 1 AND 19 AND valid.bot NOT GLOB '*[^0-9]*' AND substr(valid.bot,1,1)<>'0'
 AND (length(valid.bot)<19 OR valid.bot<='9223372036854775807' COLLATE BINARY)
 AND length(valid.chat) BETWEEN 1 AND 19 AND valid.chat NOT GLOB '*[^0-9]*' AND substr(valid.chat,1,1)<>'0'
 AND (length(valid.chat)<19 OR valid.chat<='9223372036854775807' COLLATE BINARY)
 AND CAST(valid.chat AS TEXT)=CAST(valid.actor_id AS TEXT)
 AND length(valid.message) BETWEEN 1 AND 10 AND valid.message NOT GLOB '*[^0-9]*' AND substr(valid.message,1,1)<>'0'
 AND (length(valid.message)<10 OR valid.message<='2147483647' COLLATE BINARY);
