SELECT id, user_id, domain,
       CASE WHEN octet_length(content) <= $2 THEN content ELSE NULL END AS content,
       octet_length(content) AS content_bytes, is_valid, is_pool, updated_at
FROM public.cookies
WHERE id = $1
