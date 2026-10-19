"""Separate one-shot content protocol; media main.py and jobs are unchanged."""
import json
import math
import sys
import network_guard as guard
from proxy_tunnel import proxy_route
from content_transport import ContentError, Transport
from content_extract import video_id, youtube_fetch, web_fetch

MAX_INPUT = 32768
MAX_CHARS = 200000


def validate(request):
    if not isinstance(request, dict) or type(request.get('version')) is not int or request.get('version') != 1:
        raise ContentError('invalid_request')
    url = request.get('url')
    if not isinstance(url, str) or len(url) > 4096:
        raise ContentError('invalid_request')
    guard._parse_http_url(url, 'invalid source')
    options = request.get('options')
    config = request.get('config')
    if not isinstance(options, dict) or not isinstance(config, dict):
        raise ContentError('invalid_request')
    cap = options.get('max_chars')
    if type(cap) is not int or not 1 <= cap <= MAX_CHARS or type(options.get('transcript_only')) is not bool:
        raise ContentError('invalid_request')
    languages = options.get('transcript_languages')
    if not isinstance(languages, list) or len(languages) > 32 or any(not isinstance(s, str) or not 1 <= len(s) <= 32 or not all(c.isalnum() or c == '-' for c in s) for s in languages):
        raise ContentError('invalid_request')
    token = options.get('github_token')
    if token is not None and (not isinstance(token, str) or not 1 <= len(token) <= 4096 or any(ord(c) < 33 or ord(c) > 126 for c in token)):
        raise ContentError('invalid_request')
    timeout = config.get('timeout_secs')
    bound = config.get('max_bytes')
    if type(timeout) not in (int, float) or not math.isfinite(timeout) or not 0 < timeout <= 300 or type(bound) is not int or not 1024 <= bound <= 8388608:
        raise ContentError('invalid_request')
    proxy = config.get('proxy_url')
    if proxy is not None and not isinstance(proxy, str):
        raise ContentError('invalid_request')
    return url, options, config


def run(request, *, allowed_proxy_origins=(), validate_destination=None):
    url, options, config = validate(request)
    # No source or private proxy allowlist is accepted from untrusted requests.
    with guard.guard_context((), allowed_proxy_origins, dns_timeout=config['timeout_secs']):
        if validate_destination:
            validate_destination(url)
        guard.validate_source_url(url, ())
        guard.configure_proxy(config.get('proxy_url'))
        with proxy_route(config.get('proxy_url'), config['timeout_secs']) as proxy:
            transport = Transport(config['timeout_secs'], config['max_bytes'], proxy, validate_destination)
            vid = video_id(url)
            if vid:
                text, title, seconds, via = youtube_fetch(transport, vid, options['transcript_languages'], options['transcript_only'])
            else:
                if options['transcript_only']:
                    raise ContentError('no_transcript')
                text, title, via = web_fetch(transport, url, options.get('github_token'))
                seconds = None
            if not text.strip():
                raise ContentError('no_content')
            return {'text': text[:options['max_chars']], 'title': title[:500] if isinstance(title, str) else None, 'video_id': vid, 'video_seconds': seconds, 'via': via}


def main():
    try:
        raw = sys.stdin.buffer.read(MAX_INPUT + 1)
        if len(raw) > MAX_INPUT:
            raise ContentError('invalid_request')
        result = {'version': 1, 'content': run(json.loads(raw))}
    except guard.NetworkResolutionError:
        result = {'version': 1, 'error': 'content_http_error'}
    except (guard.NetworkPolicyError, guard.NetworkValidationError):
        result = {'version': 1, 'error': 'content_network_denied'}
    except ContentError as e:
        result = {'version': 1, 'error': e.code}
    except Exception:
        # Never print exception strings, URLs, tokens, or tracebacks.
        result = {'version': 1, 'error': 'content_invalid_response'}
    sys.stdout.write(json.dumps(result, ensure_ascii=False, separators=(',', ':')) + '\n')


if __name__ == '__main__':
    main()
