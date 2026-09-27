SELECT plugin, feature, expires_at
FROM public.user_permissions
WHERE user_id = $1;
