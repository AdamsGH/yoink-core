SELECT file_id FROM file_cache WHERE cache_key=? AND file_type='audio' AND expires_at>CAST(unixepoch('subsec')*1000000 AS INTEGER);
