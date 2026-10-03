CREATE TRIGGER test_membership_reject BEFORE INSERT ON membership_users
WHEN NEW.user_id=8 BEGIN SELECT RAISE(ABORT, 'test rollback'); END;
