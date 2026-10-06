CREATE TRIGGER nsfw_fail_insert BEFORE INSERT ON runtime_operations BEGIN SELECT RAISE(ABORT,'fixture'); END;
