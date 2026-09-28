SELECT plugin, feature, expires_at
FROM user_permissions
WHERE user_id = ?1;