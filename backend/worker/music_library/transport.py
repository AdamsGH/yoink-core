"""Bounded provider bodies and every-hop DNS checks, including browser TLS."""
from urllib.parse import urljoin, urlsplit
import threading
from contextlib import contextmanager
import httpx as _httpx
import network_guard as guard
from failures import MediaFailure
from proxy_tunnel import _resolve_pinned

Timeout = _httpx.Timeout
HTTPStatusError = _httpx.HTTPStatusError
RequestError = _httpx.RequestError
_budget = None

class Budget:
    def __init__(self, timeout, maximum, requests):
        self.timeout, self.maximum, self.requests = timeout, maximum, requests
        self.lock = threading.Lock()
        self.proxy = None
    def reserve(self):
        with self.lock:
            if self.requests <= 0:
                raise MediaFailure("resource_limit")
            self.requests -= 1
    def admit(self, url):
        self.reserve()
        guard._validate_source_request(str(url))

def configure(timeout, maximum, requests):
    global _budget
    _budget = Budget(timeout, maximum, requests)

class BoundedTransport(_httpx.AsyncBaseTransport):
    def __init__(self, proxy=None):
        self.inner = _httpx.AsyncHTTPTransport(proxy=proxy, retries=0)
    async def handle_async_request(self, request):
        _budget.admit(request.url)
        request.extensions["timeout"] = {"connect": _budget.timeout, "read": _budget.timeout, "write": _budget.timeout, "pool": _budget.timeout}
        response = await self.inner.handle_async_request(request)
        data = bytearray()
        try:
            async for chunk in response.aiter_bytes():
                data.extend(chunk)
                if len(data) > _budget.maximum:
                    raise MediaFailure("resource_limit")
        finally:
            await response.aclose()
        headers = [(k, v) for k, v in response.headers.multi_items() if k.lower() not in ("content-encoding", "content-length")]
        return _httpx.Response(response.status_code, headers=headers, content=bytes(data), request=request)
    async def aclose(self):
        await self.inner.aclose()

class AsyncClient(_httpx.AsyncClient):
    def __init__(self, *args, proxy=None, timeout=None, **kwargs):
        kwargs.setdefault("headers", {"User-Agent": "Mozilla/5.0", "Accept-Language": "en-US,en;q=0.9", "Accept-Encoding": "identity"})
        super().__init__(*args, transport=BoundedTransport(proxy), timeout=_budget.timeout, trust_env=False, **kwargs)
    async def request(self, *args, **kwargs):
        kwargs["timeout"] = _budget.timeout
        return await super().request(*args, **kwargs)

class AsyncSession:
    def __init__(self, impersonate="chrome"):
        from curl_cffi.requests import AsyncSession as CurlSession
        self.session = CurlSession(impersonate=impersonate, trust_env=False)
        self.proxies = {}
    async def __aenter__(self):
        return self
    async def __aexit__(self, *args):
        await self.session.close()
    async def get(self, url, **kwargs):
        from curl_cffi import CurlOpt
        for _ in range(6):
            _budget.admit(url)
            host, port = guard._host_port(urlsplit(url))
            proxy = self.proxies.get("https") or self.proxies.get("http")
            if not proxy:
                address, _ = _resolve_pinned(host, port)
                if ":" in address:
                    address = "[" + address + "]"
                self.session.curl_options = {CurlOpt.RESOLVE: [f"{host}:{port}:{address}"]}
            response = await self.session.get(url, proxy=proxy, allow_redirects=False, timeout=_budget.timeout, stream=True)
            body = bytearray()
            try:
                async for chunk in response.aiter_content():
                    body.extend(chunk)
                    if len(body) > _budget.maximum:
                        raise MediaFailure("resource_limit")
            finally:
                await response.aclose()
            if response.status_code in (301, 302, 303, 307, 308) and response.headers.get("Location"):
                url = urljoin(url, response.headers["Location"])
                continue
            return _httpx.Response(response.status_code, content=bytes(body), request=_httpx.Request("GET", url))
        raise MediaFailure("source_unavailable")

@contextmanager
def requests_budget():
    import requests
    original = requests.Session.send
    if guard._SOURCE_WIRE_ADMISSION != _budget.reserve or (
        original is not guard._guarded_requests_send
        and getattr(original, "_source_wire_guard", None) is not guard._guarded_requests_send
    ):
        raise MediaFailure("resource_limit")
    def bounded_response(response, **kwargs):
        # Session.send inspects redirect bodies even with allow_redirects=False.
        # Bound them in the response hook, before that machinery can consume them.
        data = bytearray()
        try:
            for chunk in response.iter_content(65536):
                data.extend(chunk)
                if len(data) > _budget.maximum:
                    raise MediaFailure("resource_limit")
            response._content = bytes(data)
            response._content_consumed = True
        finally:
            response.close()
        return response
    def send(session, request, **kwargs):
        session.trust_env = False
        kwargs.update(stream=True, timeout=_budget.timeout, proxies={"http": _budget.proxy or "", "https": _budget.proxy or ""})
        # No hidden urllib3 retries may bypass Session.send admission.
        session.get_adapter(request.url).max_retries = requests.adapters.Retry.from_int(0)
        redirects = kwargs.pop("allow_redirects", True)
        hooks = request.hooks.get("response", [])
        request.hooks["response"] = [bounded_response, *(hook for hook in hooks if hook is not bounded_response)]
        try:
            response = original(session, request, allow_redirects=False, **kwargs)
        finally:
            request.hooks["response"] = hooks
        if redirects:
            history = [response, *session.resolve_redirects(response, request, **kwargs)]
            response = history[-1]
            response.history = history[:-1]
        return response
    send._source_wire_guard = guard._guarded_requests_send
    requests.Session.send = send
    try:
        yield
    finally:
        requests.Session.send = original
