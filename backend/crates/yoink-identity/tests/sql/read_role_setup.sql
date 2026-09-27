CREATE ROLE __ROLE__ NOLOGIN;
GRANT __ROLE__ TO CURRENT_USER;
GRANT USAGE ON SCHEMA public TO __ROLE__;
GRANT SELECT (id, role, language, ban_until) ON public.users TO __ROLE__;
GRANT SELECT (user_id, plugin, feature, expires_at) ON public.user_permissions TO __ROLE__;
GRANT SELECT (id, enabled, auto_grant_role) ON public.groups TO __ROLE__;
GRANT SELECT (user_id, group_id, role_override) ON public.user_group_policies TO __ROLE__;
GRANT SELECT (group_id, thread_id, enabled) ON public.thread_policies TO __ROLE__;
GRANT SELECT (key, value) ON public.bot_settings TO __ROLE__;
