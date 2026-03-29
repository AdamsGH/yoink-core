SELECT
    u.id, u.username, u.first_name, u.photo_url,
    u.role, u.language, u.theme, u.ban_until,
    u.created_at, u.updated_at,
    COALESCE(dl.dl_count, 0) AS dl_count,
    dl.dl_last_at
FROM users u
LEFT JOIN LATERAL (
    SELECT COUNT(*) AS dl_count, MAX(created_at) AS dl_last_at
    FROM download_log WHERE user_id = u.id
) dl ON true
{where}
ORDER BY {order}
LIMIT :limit OFFSET :offset
