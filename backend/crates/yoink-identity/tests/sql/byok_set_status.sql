UPDATE insight_user_byok
SET tested_at = CASE WHEN ?1 THEN CAST(strftime('%s', 'now') AS INTEGER) * 1000000 ELSE NULL END,
    test_error = CASE WHEN ?1 THEN ?2 ELSE NULL END
WHERE user_id = ?3;
