"""DNS-level SSRF guard for urllib, requests, and downloader networking."""
from __future__ import annotations

from contextlib import contextmanager
import ipaddress
import http.client
import socket
import threading
import unicodedata
from urllib.parse import urlsplit
from urllib.request import OpenerDirector, ProxyHandler


_ORIGINAL_GETADDRINFO = socket.getaddrinfo
_ORIGINAL_OPENER_OPEN = OpenerDirector.open
_ORIGINAL_PROXY_OPEN = ProxyHandler.proxy_open
_ALLOWED_ORIGINS: frozenset[tuple[str, int]] = frozenset()
_ALLOWED_PROXY_ORIGINS: frozenset[tuple[str, int]] = frozenset()
_CONFIGURED_PROXY: tuple[str, int] | None = None
_ACTIVE_RELAY_ORIGIN: tuple[str, int] | None = None
_REQUESTS_SESSION = None
_ORIGINAL_REQUESTS_SEND = None
_SOURCE_WIRE_ADMISSION = None
_INHERIT_WIRE_ADMISSION = object()
_BENCHMARK_DNS = None


_GUARD_FAILURE_COUNT = 0
_GUARD_FAILURE_LOCK = threading.Lock()


def record_guard_failure() -> None:
    """Retain policy/local-relay denials across a thread/HTTP boundary."""
    global _GUARD_FAILURE_COUNT
    with _GUARD_FAILURE_LOCK:
        _GUARD_FAILURE_COUNT += 1


def guard_failure_count() -> int:
    with _GUARD_FAILURE_LOCK:
        return _GUARD_FAILURE_COUNT


class NetworkPolicyError(OSError):
    failure_class = "network_denied"
    fallback_forbidden = True
    def __init__(self, message):
        record_guard_failure()
        super().__init__(message)


class NetworkValidationError(ValueError):
    fallback_forbidden = True
    def __init__(self, message):
        record_guard_failure()
        super().__init__(message)


class NetworkResolutionError(OSError):
    """Unavailable DNS is retryable, not an SSRF policy rejection."""
    failure_class = "source_unavailable"
    fallback_forbidden = True


def resolve_addresses(*args, **kwargs):
    try:
        try:
            answers = _ORIGINAL_GETADDRINFO(*args, **kwargs)
        except socket.gaierror:
            if _BENCHMARK_DNS is None or _is_ip_address(args[0]):
                raise
            real = _BENCHMARK_DNS.addresses(args[0])
            return [answer for address in real
                    for answer in _ORIGINAL_GETADDRINFO(address, *args[1:], **kwargs)]
        if _BENCHMARK_DNS is not None and answers:
            from benchmark_dns import BENCHMARK
            addresses = [ipaddress.ip_address(a[4][0].split('%', 1)[0]) for a in answers]
            host = args[0]
            if not _is_ip_address(host) and all(a in BENCHMARK for a in addresses):
                real = _BENCHMARK_DNS.addresses(host)
                return [answer for address in real
                        for answer in _ORIGINAL_GETADDRINFO(address, *args[1:], **kwargs)]
        return answers
    except (NetworkPolicyError, NetworkResolutionError):
        raise
    except (OSError, http.client.HTTPException) as exc:
        raise NetworkResolutionError("network destination resolution failed") from exc
    except (ValueError, KeyError, TypeError) as exc:
        raise NetworkPolicyError("network destination resolution failed") from exc


def _origin_host_port(origin: str) -> tuple[str, int]:
    parsed = _parse_http_url(origin, "invalid allowed origin")
    if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise NetworkValidationError("invalid allowed origin")
    return _host_port(parsed)


def _parse_http_url(url: str, error: str, *, allow_credentials: bool = False):
    if not isinstance(url, str) or any(unicodedata.category(char) == "Cc" for char in url):
        raise NetworkValidationError(error)
    try:
        parsed = urlsplit(url)
        if parsed.scheme.lower() not in ("http", "https") or not parsed.hostname:
            raise NetworkValidationError(error)
        if not allow_credentials and (parsed.username is not None or parsed.password is not None):
            raise NetworkValidationError(error)
        parsed.port
        return parsed
    except (ValueError, UnicodeError) as exc:
        raise NetworkValidationError(error) from exc


def _host_port(parsed) -> tuple[str, int]:
    port = parsed.port
    if port is None:
        port = 443 if parsed.scheme.lower() == "https" else 80
    return parsed.hostname.rstrip(".").lower(), port


def _address_is_public(address: str) -> bool:
    ip = ipaddress.ip_address(address.split("%", 1)[0])
    if isinstance(ip, ipaddress.IPv6Address) and ip.ipv4_mapped is not None:
        ip = ip.ipv4_mapped
    return (
        ip.is_global
        and not ip.is_private
        and not ip.is_loopback
        and not ip.is_link_local
        and not ip.is_multicast
        and not ip.is_reserved
        and not ip.is_unspecified
    )


def guarded_getaddrinfo(host, port, *args, **kwargs):
    results = resolve_addresses(host, port, *args, **kwargs)
    if not results:
        raise NetworkResolutionError("network destination resolution failed")
    normalized_host = (
        host.decode("ascii", "ignore").rstrip(".").lower()
        if isinstance(host, bytes)
        else str(host).rstrip(".").lower()
    )
    try:
        numeric_port = int(port)
    except (TypeError, ValueError):
        numeric_port = socket.getservbyname(str(port))
    source_allowlisted = (normalized_host, numeric_port) in _ALLOWED_ORIGINS
    proxy_allowlisted = (
        (normalized_host, numeric_port) in _ALLOWED_PROXY_ORIGINS
        and (normalized_host, numeric_port) == _CONFIGURED_PROXY
    )
    relay_allowlisted = (normalized_host, numeric_port) == _ACTIVE_RELAY_ORIGIN
    for result in results:
        try:
            public = _address_is_public(result[4][0])
        except (IndexError, TypeError, ValueError):
            public = False
        if not public and not source_allowlisted and not proxy_allowlisted and not relay_allowlisted:
            raise NetworkPolicyError("network destination is not permitted")
    return results


def _ensure_guarded():
    global _REQUESTS_SESSION, _ORIGINAL_REQUESTS_SEND
    if socket.getaddrinfo is not guarded_getaddrinfo:
        socket.getaddrinfo = guarded_getaddrinfo
    if OpenerDirector.open is not _guarded_opener_open:
        OpenerDirector.open = _guarded_opener_open
    if ProxyHandler.proxy_open is not _guarded_proxy_open:
        ProxyHandler.proxy_open = _guarded_proxy_open
    try:
        import requests
    except ImportError:
        return
    session_type = requests.Session
    if _REQUESTS_SESSION is not session_type:
        _REQUESTS_SESSION = session_type
        _ORIGINAL_REQUESTS_SEND = session_type.send
    if session_type.send is not _guarded_requests_send:
        session_type.send = _guarded_requests_send


def install_guard(
    allowed_origins: tuple[str, ...],
    allowed_proxy_origins: tuple[str, ...] = (),
) -> None:
    """Install (or update) the process-wide guard; proxy origins never authorize sources."""
    global _ALLOWED_ORIGINS, _ALLOWED_PROXY_ORIGINS
    _ALLOWED_ORIGINS = frozenset(_origin_host_port(item) for item in allowed_origins)
    _ALLOWED_PROXY_ORIGINS = frozenset(_origin_host_port(item) for item in allowed_proxy_origins)
    _ensure_guarded()


def configure_proxy(proxy_url: str | None) -> None:
    """Set the one explicit proxy URL permitted for requests/urllib, or disable proxying."""
    global _CONFIGURED_PROXY
    if proxy_url is None:
        _CONFIGURED_PROXY = None
        return
    parsed = _parse_http_url(proxy_url, "invalid proxy", allow_credentials=True)
    origin = _host_port(parsed)
    if _is_ip_address(origin[0]) and not _address_is_public(origin[0]) and origin not in _ALLOWED_PROXY_ORIGINS:
        raise NetworkValidationError("invalid proxy")
    _CONFIGURED_PROXY = origin


def _validate_source_request(url: str) -> None:
    validate_source_url(url, tuple(_format_origin(item) for item in _ALLOWED_ORIGINS))


def _validate_proxy_request(url: str) -> None:
    parsed = _parse_http_url(url, "invalid proxy", allow_credentials=True)
    origin = _host_port(parsed)
    if origin == _ACTIVE_RELAY_ORIGIN:
        return
    if _ACTIVE_RELAY_ORIGIN is not None or _CONFIGURED_PROXY is None or origin != _CONFIGURED_PROXY:
        raise NetworkPolicyError("network destination is not permitted")


def _guarded_opener_open(self, fullurl, data=None, timeout=socket._GLOBAL_DEFAULT_TIMEOUT):
    url = fullurl.full_url if hasattr(fullurl, "full_url") else fullurl
    if _SOURCE_WIRE_ADMISSION is not None:
        _SOURCE_WIRE_ADMISSION()
    _validate_source_request(url)
    return _ORIGINAL_OPENER_OPEN(self, fullurl, data, timeout)


def _guarded_proxy_open(self, req, proxy, type):
    _validate_proxy_request(proxy)
    return _ORIGINAL_PROXY_OPEN(self, req, proxy, type)


def _guarded_requests_send(self, request, **kwargs):
    if _SOURCE_WIRE_ADMISSION is not None:
        _SOURCE_WIRE_ADMISSION()
    _validate_source_request(request.url)
    proxies = kwargs.get("proxies") or {}
    for scheme, proxy_url in proxies.items():
        if proxy_url:
            _validate_proxy_request(proxy_url)
    return _ORIGINAL_REQUESTS_SEND(self, request, **kwargs)


def _format_origin(host_port: tuple[str, int]) -> str:
    host, port = host_port
    return f"http://[{host}]:{port}" if ":" in host else f"http://{host}:{port}"


def validate_source_url(source_url: str, allowed_origins: tuple[str, ...]) -> None:
    parsed = _parse_http_url(source_url, "invalid source")
    host, port = _host_port(parsed)
    permitted = frozenset(_origin_host_port(origin) for origin in allowed_origins)
    if _is_ip_address(host):
        if not _address_is_public(host) and (host, port) not in permitted:
            raise NetworkPolicyError("network destination is not permitted")
    else:
        results = resolve_addresses(host, port, socket.AF_UNSPEC, socket.SOCK_STREAM)
        if not results:
            raise NetworkResolutionError("network destination resolution failed")
        source_allowed = (host, port) in permitted or (host, port) in _ALLOWED_ORIGINS
        for result in results:
            try:
                public = _address_is_public(result[4][0])
            except (IndexError, TypeError, ValueError):
                public = False
            if not public and not source_allowed:
                raise NetworkPolicyError("network destination is not permitted")


def _is_ip_address(host: str) -> bool:
    try:
        ipaddress.ip_address(host)
        return True
    except ValueError:
        return False


def restore_guard() -> None:
    """Restore original networking methods and clear all guard configuration."""
    global _ALLOWED_ORIGINS, _ALLOWED_PROXY_ORIGINS, _CONFIGURED_PROXY, _ACTIVE_RELAY_ORIGIN
    global _REQUESTS_SESSION, _ORIGINAL_REQUESTS_SEND
    global _SOURCE_WIRE_ADMISSION
    global _BENCHMARK_DNS
    socket.getaddrinfo = _ORIGINAL_GETADDRINFO
    OpenerDirector.open = _ORIGINAL_OPENER_OPEN
    ProxyHandler.proxy_open = _ORIGINAL_PROXY_OPEN
    if _REQUESTS_SESSION is not None and _ORIGINAL_REQUESTS_SEND is not None:
        _REQUESTS_SESSION.send = _ORIGINAL_REQUESTS_SEND
    _REQUESTS_SESSION = None
    _ORIGINAL_REQUESTS_SEND = None
    _ALLOWED_ORIGINS = frozenset()
    _ALLOWED_PROXY_ORIGINS = frozenset()
    _CONFIGURED_PROXY = None
    _ACTIVE_RELAY_ORIGIN = None
    _SOURCE_WIRE_ADMISSION = None
    _BENCHMARK_DNS = None


@contextmanager
def guard_context(
    allowed_origins: tuple[str, ...],
    allowed_proxy_origins: tuple[str, ...] = (),
    *, source_wire_admission=_INHERIT_WIRE_ADMISSION, dns_timeout=None,
):
    """Restore enclosing policy on exit. Optional wire admission precedes source DNS.

    Omitted admission inherits the enclosing callback; explicit None disables it.
    """
    global _ALLOWED_ORIGINS, _ALLOWED_PROXY_ORIGINS, _CONFIGURED_PROXY
    global _SOURCE_WIRE_ADMISSION
    global _BENCHMARK_DNS
    previous = (_ALLOWED_ORIGINS, _ALLOWED_PROXY_ORIGINS, _CONFIGURED_PROXY, _SOURCE_WIRE_ADMISSION, _BENCHMARK_DNS)
    previous_send = _REQUESTS_SESSION.send if _REQUESTS_SESSION is not None else None
    was_installed = socket.getaddrinfo is guarded_getaddrinfo
    install_guard(allowed_origins, allowed_proxy_origins)
    if dns_timeout is not None:
        from benchmark_dns import Resolver
        _BENCHMARK_DNS = Resolver(dns_timeout)
    # Process-wide like the guard itself, including downloader fragment threads.
    if source_wire_admission is not _INHERIT_WIRE_ADMISSION:
        _SOURCE_WIRE_ADMISSION = source_wire_admission
    if _SOURCE_WIRE_ADMISSION is not None and getattr(previous_send, "_source_wire_guard", None) is _guarded_requests_send:
        _REQUESTS_SESSION.send = previous_send
    try:
        yield
    finally:
        if was_installed:
            _ALLOWED_ORIGINS, _ALLOWED_PROXY_ORIGINS, _CONFIGURED_PROXY, _SOURCE_WIRE_ADMISSION, _BENCHMARK_DNS = previous
            _ensure_guarded()
            if _SOURCE_WIRE_ADMISSION is not None and getattr(previous_send, "_source_wire_guard", None) is _guarded_requests_send:
                _REQUESTS_SESSION.send = previous_send
        else:
            restore_guard()
