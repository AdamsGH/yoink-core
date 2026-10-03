INSERT INTO thread_policies(group_id,thread_id,name,enabled) VALUES(?1,?2,?3,?4) ON CONFLICT DO UPDATE SET name=COALESCE(excluded.name,thread_policies.name),enabled=excluded.enabled;
