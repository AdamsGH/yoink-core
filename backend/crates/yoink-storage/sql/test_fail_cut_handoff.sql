CREATE TRIGGER test_fail_cut_handoff BEFORE INSERT ON runtime_cut_sessions
BEGIN SELECT RAISE(ABORT, 'test cut insert failure'); END;
