"""Bounded GET-only content transport. Every hop is validated and DNS pinned."""
import http.client
import json
import socket
import ssl
import time
from urllib.parse import urljoin, urlsplit

import network_guard as guard
from proxy_tunnel import _resolve_pinned


class ContentError(Exception):
    def __init__(self, code):
        self.code = code
        super().__init__(code)


class Transport:
    def __init__(self, timeout, max_bytes, proxy=None, validate_destination=None):
        self.validate_destination = validate_destination
        self.timeout = timeout
        self.max_bytes = max_bytes
        self.proxy = proxy
        self.deadline = time.monotonic() + timeout

    def get(self, url, token=None, headers=None):
        before = guard.guard_failure_count()
        try:
            return self._get(url, token, headers)
        except (TimeoutError, socket.timeout):
            raise ContentError('content_timeout') from None
        except Exception as exc:
            if self.proxy:
                import requests
                if isinstance(exc, requests.exceptions.Timeout):
                    raise ContentError('content_timeout') from None
            raise
        finally:
            # Relay-thread policy denials must not turn into ordinary fallback misses.
            if guard.guard_failure_count() != before:
                raise guard.NetworkPolicyError('network destination is not permitted')

    def _get(self, url, token=None, extra_headers=None):
        authenticated_origin = urlsplit(url).netloc == 'api.github.com' and urlsplit(url).scheme == 'https'
        for hop in range(6):
            guard._validate_source_request(url)
            if self.validate_destination:
                self.validate_destination(url)
            p = urlsplit(url)
            remaining = self.deadline - time.monotonic()
            if remaining <= 0:
                raise ContentError('content_timeout')
            headers = {'User-Agent': 'yoink-content/1', 'Accept-Encoding': 'identity'}
            if extra_headers:
                headers.update(extra_headers)
            # Once a redirect leaves the exact API origin, authentication is lost forever.
            authenticated_origin = authenticated_origin and p.scheme == 'https' and p.netloc == 'api.github.com'
            if authenticated_origin and token:
                headers['Authorization'] = 'Bearer ' + token
                headers['Accept'] = 'application/vnd.github+json'
            if self.proxy:
                import requests
                session = requests.Session()
                session.trust_env = False
                try:
                    with session.get(url, headers=headers, proxies={'http': self.proxy, 'https': self.proxy}, timeout=remaining, stream=True, allow_redirects=False) as r:
                        status, location = r.status_code, r.headers.get('Location')
                        if r.headers.get('Content-Encoding', 'identity').lower() != 'identity':
                            raise ContentError('content_invalid_response')
                        expected = self._length(r.headers.get('Content-Length'))
                        body = bytearray()
                        for chunk in r.iter_content(65536):
                            body.extend(chunk)
                            self._bound(body)
                        if expected is not None and len(body) != expected:
                            raise ContentError('content_invalid_response')
                finally:
                    session.close()
            else:
                host, port = guard._host_port(p)
                address = _resolve_pinned(host, port)
                conn = http.client.HTTPConnection(host, port, timeout=remaining)
                try:
                    conn.sock = socket.create_connection(address, remaining)
                    if p.scheme == 'https':
                        conn.sock = ssl.create_default_context().wrap_socket(conn.sock, server_hostname=host)
                    path = (p.path or '/') + ('?' + p.query if p.query else '')
                    conn.request('GET', path, headers=headers)
                    r = conn.getresponse()
                    status, location = r.status, r.getheader('Location')
                    if (r.getheader('Content-Encoding') or 'identity').lower() != 'identity':
                        raise ContentError('content_invalid_response')
                    expected = self._length(r.getheader('Content-Length'))
                    body = bytearray()
                    while True:
                        chunk = r.read(min(65536, self.max_bytes + 1 - len(body)))
                        if not chunk:
                            break
                        body.extend(chunk)
                        self._bound(body)
                    if expected is not None and len(body) != expected:
                        raise ContentError('content_invalid_response')
                finally:
                    conn.close()
            if status in (301, 302, 303, 307, 308) and location:
                url = urljoin(url, location)
                continue
            if status >= 400:
                raise ContentError('content_http_error')
            return bytes(body).decode('utf-8', 'replace')
        raise ContentError('content_redirect_limit')

    def _length(self, raw):
        if raw is None:
            return None
        try:
            value = int(raw)
        except (TypeError, ValueError):
            raise ContentError('content_invalid_response') from None
        if value < 0:
            raise ContentError('content_invalid_response')
        if value > self.max_bytes:
            raise ContentError('content_too_large')
        return value

    def _bound(self, body):
        if len(body) > self.max_bytes:
            raise ContentError('content_too_large')
        if time.monotonic() > self.deadline:
            raise ContentError('content_timeout')

    def json(self, url, token=None):
        try:
            return json.loads(self.get(url, token))
        except ValueError:
            raise ContentError('content_invalid_response') from None
