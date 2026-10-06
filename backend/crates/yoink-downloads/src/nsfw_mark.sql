-- Recheck role and ban at the mutation boundary; repeated mark means one pending mark.
INSERT INTO runtime_nsfw_marks(actor_id)
SELECT id FROM users
WHERE id = ?1 AND (role IN ('user', 'moderator', 'admin', 'owner') OR (?3 = 1 AND role = 'restricted'))
AND (ban_until IS NULL OR ban_until <= ?2)
ON CONFLICT(actor_id) DO UPDATE SET actor_id = excluded.actor_id
RETURNING actor_id;
