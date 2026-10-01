UPDATE runtime_cut_sessions SET stage='confirming', operation_id=NULL
WHERE actor_id=?1 AND bot_id=?2 AND chat_id=?3 AND id=?4;
