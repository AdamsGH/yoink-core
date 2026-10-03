SELECT EXISTS (
    SELECT 1 FROM groups g
    WHERE g.id = ?2 AND g.enabled = 1
      AND NOT EXISTS (SELECT 1 FROM membership_group_suppressions s WHERE s.group_id = g.id)
      AND NOT EXISTS (SELECT 1 FROM membership_bots b WHERE b.source_id = ?1 AND b.group_id = g.id AND b.present = 0)
      AND NOT EXISTS (
          SELECT 1 FROM membership_users m
          LEFT JOIN membership_bots b ON b.source_id=m.source_id AND b.group_id=m.group_id
          WHERE m.source_id = ?1 AND m.group_id=g.id AND m.user_id=?3
            AND (m.present=0 OR (b.generation IS NOT NULL AND m.generation != b.generation))
      )
);
