SELECT 1 WHERE COALESCE((
    SELECT message_id FROM runtime_personal_cookie_mutation_watermarks WHERE actor_id=?1
),0)<?2;
