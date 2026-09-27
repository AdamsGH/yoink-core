SELECT
    (SELECT enabled FROM public.thread_policies
     WHERE group_id = $1 AND thread_id IS NOT DISTINCT FROM $2) AS matching_enabled,
    EXISTS (SELECT 1 FROM public.thread_policies
            WHERE group_id = $1 AND enabled) AS has_enabled_policy;
