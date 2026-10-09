SELECT source_platform AS platform,count(*) AS count FROM music_resolve_log WHERE user_id=? GROUP BY source_platform ORDER BY count DESC,source_platform LIMIT 10;
