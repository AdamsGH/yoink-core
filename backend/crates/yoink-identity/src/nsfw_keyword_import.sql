INSERT INTO nsfw_keywords (keyword, note, created_at) VALUES (?1, ?2, CAST(unixepoch('subsec')*1000000 AS INTEGER)) ON CONFLICT(keyword) DO NOTHING;
