"""Anonymous primitives only. Fallback policy belongs to native Inbox."""
from urllib.parse import urlsplit
import network_guard as guard
from proxy_tunnel import proxy_route
from content_transport import Transport
from content_extract import HTMLText

MAX_BYTES = 128 * 1024
HEADERS = {'Accept': 'text/markdown', 'X-No-Cache': 'true', 'X-Return-Format': 'markdown'}


def validate(r):
    from main import _UUID_RE
    if set(r) != {'version', 'operation_id', 'attempt', 'task', 'proxy_url'} or type(r['version']) is not int or r['version'] != 2 or not isinstance(r['operation_id'], str) or not _UUID_RE.fullmatch(r['operation_id']) or int(r['operation_id'].replace('-', ''), 16) == 0 or type(r['attempt']) is not int or not 1 <= r['attempt'] <= 4294967295:
        raise ValueError('invalid content request')
    t = r['task']
    kind = t['kind']
    fields = {'content_get': {'kind', 'url', 'headers'}, 'content_readability': {'kind', 'url', 'html'}, 'content_trafilatura': {'kind', 'html'}}
    if kind not in fields or set(t) != fields[kind]:
        raise ValueError('invalid content task')
    if 'url' in t:
        if not isinstance(t['url'], str) or len(t['url'].encode()) > 8192:
            raise ValueError('invalid content URL')
        guard._parse_http_url(t['url'], 'invalid source')
    if 'html' in t and (not isinstance(t['html'], str) or len(t['html'].encode()) > MAX_BYTES):
        raise ValueError('invalid HTML')
    if 'headers' in t and (not isinstance(t['headers'], dict) or any(k not in HEADERS or HEADERS[k] != v for k, v in t['headers'].items())):
        raise ValueError('invalid headers')
    if r['proxy_url'] is not None:
        if not isinstance(r['proxy_url'], str) or len(r['proxy_url']) > 4096:
            raise ValueError('invalid proxy')
        p = guard._parse_http_url(r['proxy_url'], 'invalid proxy', allow_credentials=True)
        if p.path not in ('', '/') or p.query or p.fragment:
            raise ValueError('invalid proxy')
    return r


def validate_destination(url):
    p = guard._parse_http_url(url, 'invalid source')
    if p.hostname in ('r.jina.ai', 'curl.md'):
        # Providers are not an SSRF escape hatch, even after a redirect.
        if p.scheme != 'https' or p.netloc not in ('r.jina.ai', 'curl.md'):
            raise guard.NetworkPolicyError('provider denied')
        original = p.path[1:] + ('?' + p.query if p.query else '')
        if urlsplit(original).hostname in ('r.jina.ai', 'curl.md'):
            raise guard.NetworkPolicyError('nested provider denied')
        guard.validate_source_url(original, ())
    guard._validate_source_request(url)


def run(r, settings):
    t = r['task']
    with guard.guard_context((), settings.allowed_proxy_origins, dns_timeout=settings.socket_timeout_secs):
        guard.configure_proxy(r['proxy_url'])
        if t['kind'] == 'content_get':
            validate_destination(t['url'])
            with proxy_route(r['proxy_url'], settings.socket_timeout_secs) as proxy:
                transport = Transport(settings.socket_timeout_secs, MAX_BYTES, proxy, validate_destination)
                return {'text': transport.get(t['url'], headers=t['headers']), 'title': None}
        if t['kind'] == 'content_readability':
            from readability import Document
            doc = Document(t['html'], url=t['url'])
            parser = HTMLText()
            parser.feed(doc.summary())
            return {'text': '\n'.join(parser.all), 'title': doc.title()}
        import trafilatura
        # Trafilatura 2.0.0 retains navigation on short pages. Its precision mode
        # duplicates paragraphs; prune semantic chrome through its public API.
        text = trafilatura.extract(
            t['html'], include_comments=False, include_tables=True,
            prune_xpath=['//nav', '//footer', '//*[@role="navigation"]'],
        )
        return {'text': text or '', 'title': None}


def execute(r, settings, emit, prefix):
    try:
        result = run(r, settings)
        if len(result['text'].encode()) > MAX_BYTES or (result['title'] is not None and len(result['title'].encode()) > 2000):
            raise ValueError('result too large')
        event = {'type': 'content_completed', 'result': result}
    except guard.NetworkResolutionError:
        event = {'type': 'failure', 'class': 'source_unavailable'}
    except (guard.NetworkPolicyError, guard.NetworkValidationError):
        event = {'type': 'content_denied'}
    except Exception:
        event = {'type': 'failure', 'class': 'processing_failed'}
    emit({**prefix, 'event': event})
