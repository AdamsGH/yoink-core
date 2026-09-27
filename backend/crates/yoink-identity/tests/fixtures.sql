-- Fixture columns are explicit against the Alembic schema, including Python-only defaults.
INSERT INTO public.users
    (id, role, language, theme, is_premium, created_at, updated_at, ban_until)
VALUES
    (8000000000011, 'user', 'en', 'macchiato', false, now(), now(), NULL),
    (8000000000012, 'restricted', 'ru', 'macchiato', false, now(), now(), NULL),
    (8000000000013, 'banned', 'en', 'macchiato', false, now(), now(), NULL),
    (8000000000014, 'user', 'en', 'macchiato', false, now(), now(), now() + interval '1 hour'),
    (8000000000015, 'user', 'en', 'macchiato', false, now(), now(), NULL),
    (8000000000016, 'user', 'en', 'macchiato', false, now(), now(), NULL),
    (8000000000017, 'user', 'en', 'macchiato', false, now(), now(), now() - interval '1 hour');

INSERT INTO public.user_permissions
    (user_id, plugin, feature, granted_by, granted_at, grant_source, expires_at)
VALUES
    (8000000000011, 'dl', 'inline', 8000000000011, now(), 'manual', NULL),
    (8000000000012, 'dl', 'inline', 8000000000011, now(), 'manual', now() + interval '1 hour'),
    (8000000000013, 'dl', 'inline', 8000000000011, now(), 'manual', NULL),
    (8000000000015, 'dl', 'inline', 8000000000011, now(), 'manual', now()),
    (8000000000016, 'dl', 'inline', 8000000000011, now(), 'manual', now() - interval '1 second');

INSERT INTO public.insight_user_byok
    (user_id, provider, base_url, api_key, model, models_json, models_fetched_at,
     tested_at, test_error, created_at, updated_at)
VALUES
    (8000000000011, 'openai', NULL, 'integration-fake-key', 'fake-model', NULL, NULL,
     now(), NULL, now(), now());

INSERT INTO public.groups
    (id, enabled, auto_grant_role, allow_pm, nsfw_allowed, created_at)
VALUES
    (-8000000000011, true, 'moderator', true, false, now()),
    (-8000000000012, false, 'admin', true, false, now());

INSERT INTO public.user_group_policies
    (user_id, group_id, role_override, allow_pm_override)
VALUES
    (8000000000012, -8000000000011, 'admin', NULL);

INSERT INTO public.thread_policies (group_id, thread_id, name, enabled) VALUES
    (-8000000000011, NULL, NULL, true),
    (-8000000000011, 17, NULL, false),
    (-8000000000011, 18, NULL, true);
