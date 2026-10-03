SELECT DISTINCT m.tag
FROM membership_users m
JOIN groups g ON g.id = m.group_id AND g.enabled = 1
JOIN membership_bots b ON b.source_id = m.source_id AND b.group_id = m.group_id
WHERE m.user_id = ?1 AND m.present = 1 AND b.present = 1
  AND m.generation = b.generation AND m.tag_generation = b.generation
  AND NOT EXISTS (SELECT 1 FROM membership_bots other WHERE other.group_id=m.group_id AND other.present=0)
  AND m.tag IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM membership_group_suppressions s WHERE s.group_id = m.group_id);
