SELECT id FROM (
    SELECT id FROM groups WHERE id < 0
    UNION SELECT group_id AS id FROM membership_group_suppressions
    UNION SELECT group_id AS id FROM membership_bots
    UNION SELECT group_id AS id FROM membership_users
) WHERE id > ?1 ORDER BY id LIMIT 128;
