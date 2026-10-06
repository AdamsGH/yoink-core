UPDATE nsfw_domains SET domain=?1, note=?2 WHERE id=?3 RETURNING id;
