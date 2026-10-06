UPDATE runtime_operations SET content_failure='nsfw_blocked' WHERE id='content-failed';
UPDATE runtime_operations SET content_failure='nsfw_policy_unavailable' WHERE id='content-cancelled';
