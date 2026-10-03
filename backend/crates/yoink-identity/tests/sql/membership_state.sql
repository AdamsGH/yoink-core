SELECT
    (SELECT count(*) FROM groups) AS groups_count,
    (SELECT count(*) FROM users WHERE id=?1) AS user_count,
    (SELECT count(*) FROM membership_users WHERE user_id=?1) AS observations,
    (SELECT count(*) FROM user_permissions WHERE user_id=?1) AS ledger_count,
    (SELECT count(*) FROM membership_feature_suppressions WHERE user_id=?1) AS suppressions;
