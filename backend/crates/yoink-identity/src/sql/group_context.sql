SELECT g.enabled, g.auto_grant_role AS auto_grant_role,
       ug.role_override AS role_override
FROM groups AS g
LEFT JOIN user_group_policies AS ug
  ON ug.group_id = g.id AND ug.user_id = ?1
WHERE g.id = ?2;