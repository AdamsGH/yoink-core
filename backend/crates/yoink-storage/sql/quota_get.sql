SELECT count, reset_at FROM rate_limits WHERE user_id=?1 AND "window"=?2;
