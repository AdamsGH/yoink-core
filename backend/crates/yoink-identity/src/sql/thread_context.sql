SELECT
    (SELECT enabled FROM thread_policies
     WHERE group_id = ?1 AND thread_id IS ?2) AS matching_enabled,
    EXISTS (SELECT 1 FROM thread_policies
            WHERE group_id = ?1 AND enabled) AS has_enabled_policy;