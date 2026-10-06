INSERT INTO nsfw_keywords (keyword, note, created_at) SELECT ?1, ?2, CAST(unixepoch('subsec')*1000000 AS INTEGER) WHERE ?3 IS NULL RETURNING id;
