UPDATE runtime_progress_notices SET last_text=COALESCE(?5,last_text),finished=?6,next_attempt_at=?7
WHERE operation_id=?1 AND actor_id=?2 AND bot_id=?3 AND message_id=?4 AND finished=0
RETURNING operation_id;
