SELECT (
    g.enabled
    AND NOT EXISTS (SELECT 1 FROM membership_bots b WHERE b.group_id=g.id AND b.present=0)
    AND NOT EXISTS (
        SELECT 1 FROM membership_users m
        LEFT JOIN membership_bots b ON b.source_id=m.source_id AND b.group_id=m.group_id
        WHERE m.group_id=g.id AND m.user_id=?1
          AND (m.present=0 OR (b.generation IS NOT NULL AND m.generation != b.generation))
    )
), g.auto_grant_role AS auto_grant_role,
       ug.role_override AS role_override
FROM groups AS g
LEFT JOIN user_group_policies AS ug
  ON ug.group_id = g.id AND ug.user_id = ?1
WHERE g.id = ?2;
