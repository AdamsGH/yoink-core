SELECT (SELECT count(*) FROM runtime_operations WHERE actor_id=?1) AS operations,
       (SELECT count(*) FROM runtime_delivery_units) AS deliveries;
