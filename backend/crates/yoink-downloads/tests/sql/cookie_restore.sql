UPDATE public.cookies
SET content = '# Netscape HTTP Cookie File' || chr(10) || '.youtube.com' || chr(9) || 'TRUE' || chr(9) || '/' || chr(9) || 'FALSE' || chr(9) || '' || chr(9) || 'sid' || chr(9) || 'restored-fake-value',
    updated_at = now()
WHERE id = $1;