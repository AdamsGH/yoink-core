INSERT INTO runtime_deferred_media(actor_id,bot_id,chat_id,message_id,mode) VALUES(?1,?2,?3,?4,?5)
ON CONFLICT(actor_id,bot_id,chat_id) DO UPDATE SET message_id=excluded.message_id, mode=excluded.mode
WHERE runtime_deferred_media.message_id < excluded.message_id;
