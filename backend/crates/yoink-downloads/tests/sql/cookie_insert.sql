INSERT INTO public.cookies (user_id, domain, content, is_valid, is_pool, created_at, updated_at)
VALUES ($1, $2, $3, $4, $5, now(), now()) RETURNING id;