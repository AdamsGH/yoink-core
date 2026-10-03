INSERT INTO rate_limits(user_id,"window",count,reset_at) VALUES(?1,?2,?3,?4)
ON CONFLICT(user_id,"window") DO UPDATE SET count=excluded.count,reset_at=excluded.reset_at;
