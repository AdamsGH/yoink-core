SELECT id,title,enabled,auto_grant_role,allow_pm,nsfw_allowed,storage_chat_id,storage_thread_id,photo_url,created_at FROM groups WHERE id<0 ORDER BY id;
