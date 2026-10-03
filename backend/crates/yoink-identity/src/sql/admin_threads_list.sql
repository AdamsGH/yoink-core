SELECT id,group_id,thread_id,name,enabled FROM thread_policies WHERE group_id=?1 ORDER BY thread_id;
