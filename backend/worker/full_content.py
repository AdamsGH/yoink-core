"""Reviewed v2 correlated wrapper for the real standalone full fetch engine."""
import json
import re
import socket
from urllib.parse import urlsplit
import content_main
import network_guard as guard
from content_transport import ContentError

ERRORS = frozenset(('no_content', 'no_transcript', 'content_network_denied',
    'content_timeout', 'content_too_large', 'content_http_error',
    'content_redirect_limit', 'invalid_request', 'content_invalid_response'))
MAX_WIRE = 1024 * 1024
UUID = re.compile(r'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')


def token_relevant(url):
    p = urlsplit(url)
    return p.scheme == 'https' and p.hostname in ('github.com', 'api.github.com') and p.port in (None, 443)


def validate(r):
    if (not isinstance(r, dict) or set(r) != {'version', 'operation_id', 'attempt', 'task', 'url', 'options', 'timeout_secs', 'max_response_bytes', 'proxy_url'}
            or type(r['version']) is not int or r['version'] != 2
            or not isinstance(r['operation_id'], str) or not UUID.fullmatch(r['operation_id'])
            or r['operation_id'].lower() == '00000000-0000-0000-0000-000000000000'
            or type(r['attempt']) is not int or not 1 <= r['attempt'] <= 0xffffffff
            or r['task'] != {'kind': 'full_content_fetch'}):
        raise ValueError('invalid request')
    o = r['options']
    if not isinstance(o, dict) or set(o) - {'transcript_only', 'transcript_languages', 'max_chars', 'github_token'} or not {'transcript_only', 'transcript_languages', 'max_chars'}.issubset(o):
        raise ValueError('invalid request')
    v1 = legacy(r)
    content_main.validate(v1)
    if o.get('github_token') is not None and not token_relevant(r['url']):
        raise ValueError('invalid request')
    from network_guard import _parse_http_url
    proxy = r['proxy_url']
    if proxy is not None:
        if not isinstance(proxy, str) or not proxy or len(proxy.encode()) > 4096 or any(ord(c) < 32 for c in proxy):
            raise ValueError('invalid request')
        p = _parse_http_url(proxy, 'invalid request', allow_credentials=True)
        if p.path not in ('', '/') or p.query or p.fragment:
            raise ValueError('invalid request')
    return r


def legacy(r):
    return {'version': 1, 'url': r['url'], 'options': r['options'], 'config': {
        'timeout_secs': r['timeout_secs'], 'max_bytes': r['max_response_bytes'], 'proxy_url': r['proxy_url']}}


def execute(r, settings, emit, prefix):
    try:
        validate(r)  # Independent of host encoding and dispatcher admission.
        from content_stage import validate_destination
        result = content_main.run(legacy(r), allowed_proxy_origins=settings.allowed_proxy_origins, validate_destination=validate_destination)
        payload = {**prefix, 'event': {'type': 'full_content_completed', 'result': result}}
        # Single terminal within the EXISTING shared bound; never truncate JSON.
        if len(json.dumps(payload, separators=(',', ':'), ensure_ascii=True, allow_nan=False).encode()) > MAX_WIRE:
            raise ContentError('content_too_large')
    except guard.NetworkResolutionError:
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': 'content_http_error'}}
    except (guard.NetworkPolicyError, guard.NetworkValidationError):
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': 'content_network_denied'}}
    except (TimeoutError, socket.timeout):
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': 'content_timeout'}}
    except ContentError as e:
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': e.code if e.code in ERRORS else 'content_invalid_response'}}
    except ValueError:
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': 'invalid_request'}}
    except Exception:
        payload = {**prefix, 'event': {'type': 'full_content_failed', 'error': 'content_invalid_response'}}
    emit(payload)
