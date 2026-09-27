SELECT
    has_column_privilege(current_user, 'public.insight_user_byok', 'api_key', 'SELECT') AS api_key,
    has_column_privilege(current_user, 'public.insight_user_byok', 'base_url', 'SELECT') AS base_url,
    has_column_privilege(current_user, 'public.insight_user_byok', 'model', 'SELECT') AS model;
