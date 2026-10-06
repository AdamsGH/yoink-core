SELECT name,sql FROM sqlite_master WHERE type='index' AND tbl_name IN ('runtime_operations','runtime_delivery_units','runtime_progress_notices') ORDER BY name;
