INSERT INTO download_log(user_id,url,domain,status,created_at)
VALUES(42,'https://example.com/old','example.com','ok',1);
INSERT INTO runtime_operations(id,actor_id,idempotency_key,request,status,max_attempts,created_at,updated_at,failure_code)
VALUES('00000000-0000-4000-8000-000000000001',42,'history-fixture',
'{"actor_id":42,"source_url":"https://x.com/example/status/123","media_mode":"video","target":{"bot_id":123456,"chat_id":42}}',
'failed',1,1792368000000000,1792368000000000,'media_timeout');
