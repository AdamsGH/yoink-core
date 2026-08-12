CREATE OR REPLACE VIEW stats_user_latest_name AS
SELECT DISTINCT ON (user_id)
    user_id,
    username,
    display_name,
    date
FROM stats_user_names
ORDER BY user_id, date DESC;
