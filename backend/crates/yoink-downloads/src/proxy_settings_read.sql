SELECT COALESCE(s.proxy_enabled, 0) AS proxy_enabled,
       CASE WHEN length(CAST(s.proxy_url AS BLOB)) <= ?4 THEN s.proxy_url ELSE NULL END AS proxy_url,
       CASE WHEN length(CAST(s.proxy_url AS BLOB)) > ?4 THEN 1 ELSE 0 END AS proxy_url_too_large,
       COALESCE(s.updated_at, 0) AS revision
FROM users u LEFT JOIN dl_user_settings s ON s.user_id = u.id
WHERE u.id = ?1 AND (u.role IN ('user', 'moderator', 'admin', 'owner') OR (?3 = 1 AND u.role = 'restricted'))
  AND (u.ban_until IS NULL OR u.ban_until <= ?2);
