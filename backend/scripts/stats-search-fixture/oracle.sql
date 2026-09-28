-- Synthetic messages only, inside the disposable 54-migration PostgreSQL fixture.
INSERT INTO stats_messages (id, message_id, chat_id, date, text, caption, msg_type)
SELECT m.id, m.id, -555, TIMESTAMP '2024-01-01 00:00:00', m.text, m.caption, 'text'
FROM jsonb_to_recordset((__CORPUS__::jsonb)->'messages') AS m(id integer, text text, caption text);

SELECT jsonb_agg(jsonb_build_object('label', q.value->>'label', 'ids', (
    SELECT COALESCE(jsonb_agg(m.id ORDER BY m.id), '[]'::jsonb)
    FROM stats_messages m
    WHERE m.chat_id = -555
      AND (q.value->>'query' = '' OR m.text_search @@ plainto_tsquery('simple', q.value->>'query'))
)) ORDER BY q.ordinality)
FROM jsonb_array_elements((__CORPUS__::jsonb)->'queries') WITH ORDINALITY AS q(value, ordinality);
