INSERT INTO nsfw_domains (domain, note, created_at) VALUES (?1, ?2, CAST(unixepoch('subsec')*1000000 AS INTEGER)) ON CONFLICT(domain) DO NOTHING;
