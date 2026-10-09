SELECT count(*) AS total,coalesce(sum(created_at>=?2),0) AS this_week,coalesce(sum(created_at>=?3),0) AS today FROM music_resolve_log WHERE user_id=?1;
