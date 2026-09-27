SELECT
    (SELECT count(*) FROM yoink_runtime.operations WHERE status IN ('queued', 'running', 'delivering')) AS global_count,
    (SELECT count(*) FROM yoink_runtime.operations WHERE actor_id = $1 AND status IN ('queued', 'running', 'delivering')) AS actor_count;