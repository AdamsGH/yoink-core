UPDATE yoink_runtime.operations SET request = jsonb_set(request, '{source_url}', '"javascript:alert(1)"'::jsonb) WHERE id = $1;
