UPDATE nsfw_keywords SET keyword=?1, note=?2 WHERE id=?3 RETURNING id;
