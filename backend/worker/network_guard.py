"""DNS-level SSRF guard for yt-dlp's Python HTTP networking."""
from __future__ import annotations

import ipaddress
import socket
from urllib.parse import urlsplit


_ORIGINAL_GETADDRINFO = socket.getaddrinfo
_ALLOWED_ORIGINS: frozenset[tuple[str, int]] = frozenset()


def _origin_host_port(origin: str) -> tuple[str, int]:
    parsed = urlsplit(origin)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("invalid allowed origin")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("invalid allowed origin")
    if parsed.path not in ("", "/") or parsed.query or parsed.fragment:
        raise ValueError("invalid allowed origin")
    try:
        port = parsed.port
    except ValueError as exc:
        raise ValueError("invalid allowed origin") from exc
    if port is None:
        port = 443 if parsed.scheme == "https" else 80
    return parsed.hostname.rstrip(".").lower(), port


def _is_allowed_private(host: str, port: int) -> bool:
    normalized_host = host.rstrip(".").lower()
    return (normalized_host, port) in _ALLOWED_ORIGINS


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
    results = _ORIGINAL_GETADDRINFO(host, port, *args, **kwargs)
    if isinstance(host, bytes):
        normalized_host = host.decode("ascii", "ignore").rstrip(".").lower()
    else:
        normalized_host = str(host).rstrip(".").lower()
    try:
        numeric_port = int(port)
    except (TypeError, ValueError):
        numeric_port = socket.getservbyname(str(port))
    permitted_private_origin = _is_allowed_private(normalized_host, numeric_port)
    for result in results:
        address = result[4][0]
        if not _address_is_public(address) and not permitted_private_origin:
            raise OSError("network destination is not permitted")
    return results


def install_guard(allowed_origins: tuple[str, ...]) -> None:
    global _ALLOWED_ORIGINS
    _ALLOWED_ORIGINS = frozenset(_origin_host_port(item) for item in allowed_origins)
    socket.getaddrinfo = guarded_getaddrinfo


def validate_source_url(source_url: str, allowed_origins: tuple[str, ...]) -> None:
    parsed = urlsplit(source_url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("invalid source")
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("invalid source")
    try:
        port = parsed.port or (443 if parsed.scheme == "https" else 80)
    except ValueError as exc:
        raise ValueError("invalid source") from exc
    host = parsed.hostname.rstrip(".").lower()
    permitted = {_origin_host_port(origin) for origin in allowed_origins}
    # Resolve now so unsafe numeric literals fail before yt-dlp starts. Every later
    # connection and redirect is independently filtered by guarded_getaddrinfo.
    if _is_ip_address(host):
        if not _address_is_public(host) and (host, port) not in permitted:
            raise OSError("network destination is not permitted")
    else:
        guarded_getaddrinfo(host, port, socket.AF_UNSPEC, socket.SOCK_STREAM)


def _is_ip_address(host: str) -> bool:
    try:
        ipaddress.ip_address(host)
        return True
    except ValueError:
        return False
