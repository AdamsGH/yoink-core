SELECT id, user_id, domain, is_pool, updated_at
FROM public.cookies
WHERE is_valid = true
  AND ((user_id = $1 AND is_pool = false) OR ($3::boolean AND is_pool = true))
  AND octet_length(domain) <= 253
  AND (
    CASE lower(regexp_replace(domain, '^www[.]', '', 'i'))
      WHEN 'youtu.be' THEN 'youtube.com'
      WHEN 'm.youtube.com' THEN 'youtube.com'
      WHEN 'music.youtube.com' THEN 'youtube.com'
      WHEN 'x.com' THEN 'twitter.com'
      WHEN 'm.twitter.com' THEN 'twitter.com'
      WHEN 'm.instagram.com' THEN 'instagram.com'
      WHEN 'm.tiktok.com' THEN 'tiktok.com'
      WHEN 'm.facebook.com' THEN 'facebook.com'
      WHEN 'm.reddit.com' THEN 'reddit.com'
      WHEN 'old.reddit.com' THEN 'reddit.com'
      ELSE lower(regexp_replace(domain, '^www[.]', '', 'i'))
    END = $2
    OR right($2, length(CASE lower(regexp_replace(domain, '^www[.]', '', 'i'))
      WHEN 'youtu.be' THEN 'youtube.com'
      WHEN 'm.youtube.com' THEN 'youtube.com'
      WHEN 'music.youtube.com' THEN 'youtube.com'
      WHEN 'x.com' THEN 'twitter.com'
      WHEN 'm.twitter.com' THEN 'twitter.com'
      WHEN 'm.instagram.com' THEN 'instagram.com'
      WHEN 'm.tiktok.com' THEN 'tiktok.com'
      WHEN 'm.facebook.com' THEN 'facebook.com'
      WHEN 'm.reddit.com' THEN 'reddit.com'
      WHEN 'old.reddit.com' THEN 'reddit.com'
      ELSE lower(regexp_replace(domain, '^www[.]', '', 'i'))
    END) + 1) = '.' || CASE lower(regexp_replace(domain, '^www[.]', '', 'i'))
      WHEN 'youtu.be' THEN 'youtube.com'
      WHEN 'm.youtube.com' THEN 'youtube.com'
      WHEN 'music.youtube.com' THEN 'youtube.com'
      WHEN 'x.com' THEN 'twitter.com'
      WHEN 'm.twitter.com' THEN 'twitter.com'
      WHEN 'm.instagram.com' THEN 'instagram.com'
      WHEN 'm.tiktok.com' THEN 'tiktok.com'
      WHEN 'm.facebook.com' THEN 'facebook.com'
      WHEN 'm.reddit.com' THEN 'reddit.com'
      WHEN 'old.reddit.com' THEN 'reddit.com'
      ELSE lower(regexp_replace(domain, '^www[.]', '', 'i'))
    END
  )
ORDER BY is_pool ASC, id ASC
LIMIT $4
