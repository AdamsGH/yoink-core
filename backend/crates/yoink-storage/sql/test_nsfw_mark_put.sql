INSERT INTO runtime_nsfw_marks(actor_id) VALUES (?1) ON CONFLICT(actor_id) DO NOTHING;
