SELECT id, role::text AS role, language, ban_until
FROM public.users
WHERE id = $1;
