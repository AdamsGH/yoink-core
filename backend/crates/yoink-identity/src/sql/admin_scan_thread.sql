INSERT INTO thread_policies(group_id,thread_id,name,enabled) VALUES(?1,?2,?3,1)
ON CONFLICT DO UPDATE SET name=excluded.name;
