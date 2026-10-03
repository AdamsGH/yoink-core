SELECT id,thread_id,name,enabled FROM thread_policies WHERE group_id=?1 ORDER BY COALESCE(thread_id,0);
