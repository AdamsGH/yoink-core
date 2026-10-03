SELECT occurred_at, update_id, payload FROM membership_users WHERE source_id=?1 AND group_id=?2 AND user_id=?3;
