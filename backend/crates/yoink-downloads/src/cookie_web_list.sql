SELECT id,user_id,domain,is_valid,is_pool,label,avatar_url,validated_at,created_at,updated_at,
 substr(content,1,10)='__oauth2__' AS is_oauth,
 (?2=0 AND is_pool=1) AS inherited
FROM cookies c
WHERE (?2=1) OR (?2=2 AND is_pool=1)
 OR (?2=0 AND ((user_id=?1 AND is_pool=0) OR
 (?3=1 AND is_pool=1 AND is_valid=1 AND id=(SELECT MIN(id) FROM cookies WHERE is_pool=1 AND is_valid=1 AND domain=c.domain))))
ORDER BY domain, is_pool DESC, id;
