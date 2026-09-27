INSERT INTO public.cookies (user_id, domain, content, is_valid, is_pool, created_at, updated_at)
SELECT $1, 'capacity.test', '# Netscape HTTP Cookie File', true, true, now(), now()
FROM generate_series(1, 129);