SELECT g.enabled, g.auto_grant_role::text AS auto_grant_role,
       ug.role_override::text AS role_override
FROM public.groups AS g
LEFT JOIN public.user_group_policies AS ug
  ON ug.group_id = g.id AND ug.user_id = $1
WHERE g.id = $2;
