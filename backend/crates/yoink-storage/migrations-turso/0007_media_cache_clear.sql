CREATE TABLE runtime_media_cache_clear_watermarks (
 actor_id INTEGER NOT NULL CHECK(actor_id>0),
 bot_id INTEGER NOT NULL CHECK(bot_id>0),
 message_id INTEGER NOT NULL CHECK(message_id>0 AND message_id<=2147483647),
 PRIMARY KEY(actor_id,bot_id)
) STRICT;
CREATE INDEX operations_actor_bot_source_url ON runtime_operations(
 actor_id,
 json_extract(request,'$.target.bot_id'),
 json_extract(request,'$.source_url')
);
