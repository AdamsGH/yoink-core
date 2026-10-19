WITH history AS (
 SELECT CAST(d.id AS TEXT) AS id, d.user_id, d.url, d.domain, d.title, d.quality, d.file_size,
 d.duration, d.file_count, d.status, d.error_msg, d.group_id, g.title AS group_title,
 d.thread_id, d.message_id, d.clip_start, d.clip_end, d.created_at,
 CASE WHEN d.clip_start IS NOT NULL OR d.clip_end IS NOT NULL THEN 'clip'
 WHEN d.domain IN ('soundcloud.com','bandcamp.com','music.yandex.ru','music.yandex.com','open.spotify.com','music.apple.com','deezer.com','tidal.com','last.fm','audiomack.com','mixcloud.com') THEN 'audio'
 ELSE 'video' END AS media_type
 FROM download_log d LEFT JOIN groups g ON g.id=d.group_id WHERE d.user_id=?1
 UNION ALL
 SELECT o.id, o.actor_id, json_extract(o.request,'$.source_url'),
 lower(substr(substr(json_extract(o.request,'$.source_url'),instr(json_extract(o.request,'$.source_url'),'://')+3),1,instr(substr(json_extract(o.request,'$.source_url'),instr(json_extract(o.request,'$.source_url'),'://')+3)||'/','/')-1)),
 NULL, CAST(json_extract(o.request,'$.params.max_height') AS TEXT), NULL, NULL,
 o.planned_artifact_count,
 CASE WHEN o.status='succeeded' THEN 'ok' WHEN o.status IN ('failed','cancelled','delivery_unknown','delivery_partial') THEN 'error' ELSE o.status END,
 o.failure_code, json_extract(o.request,'$.target.chat_id'), g.title,
 json_extract(o.request,'$.target.thread_id'), o.delivered_message_id,
 json_extract(o.request,'$.clip_range.start_seconds'), json_extract(o.request,'$.clip_range.end_seconds'), o.created_at,
 CASE WHEN json_extract(o.request,'$.clip_range') IS NOT NULL OR json_array_length(o.request,'$.params.clips')>0 THEN 'clip'
 WHEN json_extract(o.request,'$.media_mode')='audio' THEN 'audio'
 WHEN json_extract(o.request,'$.media_mode')='image' THEN 'gallery' ELSE 'video' END
 FROM runtime_operations o LEFT JOIN groups g ON g.id=json_extract(o.request,'$.target.chat_id')
 WHERE o.actor_id=?1 AND o.status IN ('succeeded','failed','cancelled','delivery_unknown','delivery_partial')
), filtered AS (
 SELECT * FROM history WHERE (?2 IS NULL OR domain=?2)
 AND (?3 IS NULL OR instr(lower(url),lower(?3))>0 OR instr(lower(title),lower(?3))>0)
 AND (?4 IS NULL OR status=?4) AND (?5 IS NULL OR created_at>=?5)
 AND (?6 IS NULL OR media_type=?6)
)
