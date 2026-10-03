INSERT INTO membership_feature_suppressions (user_id,plugin,feature) VALUES (?1,?2,?3) ON CONFLICT DO NOTHING;
