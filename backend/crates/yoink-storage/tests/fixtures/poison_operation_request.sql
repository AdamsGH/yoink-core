UPDATE runtime_operations SET request = json_set(request, '$.source_url', 'javascript:alert(1)') WHERE id = ?1;
