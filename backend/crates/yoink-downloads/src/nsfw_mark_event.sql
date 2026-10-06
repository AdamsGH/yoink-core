-- Command/callback retries cannot re-arm after consumption. Telegram message IDs
-- are monotonic within a chat, not across chats or bots.
INSERT INTO runtime_nsfw_mark_events(actor_id, bot_id, chat_id, message_id)
SELECT id, ?4, ?5, ?6 FROM users
WHERE id = ?1 AND (role IN ('user', 'moderator', 'admin', 'owner') OR (?3 = 1 AND role = 'restricted'))
AND (ban_until IS NULL OR ban_until <= ?2)
ON CONFLICT(actor_id, bot_id, chat_id) DO UPDATE SET message_id = excluded.message_id
WHERE runtime_nsfw_mark_events.message_id < excluded.message_id
RETURNING actor_id;
