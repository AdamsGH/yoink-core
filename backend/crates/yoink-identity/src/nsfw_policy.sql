SELECT COALESCE((SELECT nsfw_blur FROM dl_user_settings WHERE user_id=?1),1), (SELECT nsfw_allowed FROM groups WHERE id=?2);
