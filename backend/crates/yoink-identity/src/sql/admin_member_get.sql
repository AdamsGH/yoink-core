SELECT user_id,group_id,role_override,allow_pm_override FROM user_group_policies WHERE group_id=?1 AND user_id=?2;
