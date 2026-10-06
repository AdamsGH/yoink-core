DELETE FROM runtime_nsfw_marks WHERE actor_id = ?1 RETURNING actor_id;
