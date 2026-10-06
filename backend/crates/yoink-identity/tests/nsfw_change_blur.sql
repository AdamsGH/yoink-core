INSERT INTO dl_user_settings(user_id,nsfw_blur,created_at,updated_at) VALUES(44,?1,1000000,1000000)
ON CONFLICT(user_id) DO UPDATE SET nsfw_blur=excluded.nsfw_blur;
