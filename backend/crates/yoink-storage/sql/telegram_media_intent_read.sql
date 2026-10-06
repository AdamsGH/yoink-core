SELECT i.intent FROM runtime_telegram_media_intents i JOIN runtime_operations o ON o.id=i.operation_id WHERE o.actor_id=?1 AND o.idempotency_key=?2;
