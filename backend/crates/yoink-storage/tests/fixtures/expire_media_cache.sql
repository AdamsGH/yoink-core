UPDATE runtime_media_cache SET expires_at = CAST(unixepoch('subsec') * 1000000 AS INTEGER) - 1000000 WHERE revision = ?1;
