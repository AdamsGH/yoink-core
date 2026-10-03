SELECT id,group_id,thread_id,name,enabled FROM thread_policies WHERE group_id=?1 AND thread_id IS ?2;
