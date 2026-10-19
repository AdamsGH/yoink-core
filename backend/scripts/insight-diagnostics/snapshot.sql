SELECT state,receipt,
 json_extract(identity,'$.chat') AS chat_id,
 json_extract(identity,'$.message') AS input_message_id,
 json_extract(identity,'$.actor') AS actor_id,
 json_extract(identity,'$.payload.application.incoming') AS _input,
 json_extract(identity,'$.text') AS _body
FROM runtime_text_effects
WHERE json_valid(identity)
 AND json_extract(identity,'$.payload.application.incoming') LIKE '/tldr %'
 AND (:match='' OR instr(json_extract(identity,'$.text'),:match)>0)
 AND (:chat_id IS NULL OR (json_extract(identity,'$.chat')=:chat_id
      AND json_extract(identity,'$.message')=:input_message_id))
ORDER BY rowid DESC
LIMIT 100;
