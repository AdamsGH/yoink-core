INSERT INTO public.user_permissions (user_id, plugin, feature, granted_by, granted_at, grant_source, expires_at)
VALUES ($1, 'dl', 'shared_cookies', $1, now(), 'manual', NULL);