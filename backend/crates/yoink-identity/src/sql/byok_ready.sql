SELECT EXISTS (
    SELECT 1
    FROM public.insight_user_byok
    WHERE user_id = $1
      AND tested_at IS NOT NULL
      AND test_error IS NULL
) AS ready;
