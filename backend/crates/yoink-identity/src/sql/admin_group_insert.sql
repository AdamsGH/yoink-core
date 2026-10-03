INSERT INTO groups(id,title,auto_grant_role,allow_pm,nsfw_allowed,created_at) VALUES(?1,?2,?3,?4,?5,CAST(unixepoch('subsec')*1000000 AS INTEGER));
