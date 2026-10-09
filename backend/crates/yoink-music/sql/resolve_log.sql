INSERT INTO music_resolve_log(user_id,source_platform,artist,title,platforms_found,created_at) VALUES(?,?,?,?,?,CAST(unixepoch('subsec')*1000000 AS INTEGER));
