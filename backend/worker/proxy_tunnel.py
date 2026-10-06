"""DNS-pinned HTTP CONNECT relay for worker downloader traffic."""
from __future__ import annotations

from contextlib import contextmanager
import base64
import ipaddress
import re
import select
import socket
import socketserver
import ssl
import threading
import time
from urllib.parse import unquote

import network_guard


_MAX_HEADER = 32 * 1024
_MAX_WORKERS = 16
_BUFFER_SIZE = 64 * 1024
_active_lock = threading.RLock()
_TOKEN = re.compile(rb"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$")


def _resolve_pinned(host: str, port: int, *, proxy: bool = False) -> tuple[str, int]:
    if network_guard._is_ip_address(host):
        addresses = [host]
    else:
        answers = network_guard.resolve_addresses(host, port, socket.AF_UNSPEC, socket.SOCK_STREAM)
        if not answers:
            raise network_guard.NetworkPolicyError("network destination is not permitted")
        addresses = []
        for answer in answers:
            try:
                address = answer[4][0].split("%", 1)[0]
                public = network_guard._address_is_public(address)
            except (IndexError, TypeError, ValueError):
                raise network_guard.NetworkPolicyError("network destination is not permitted") from None
            origin = (host.rstrip(".").lower(), port)
            permitted = origin in network_guard._ALLOWED_ORIGINS or (
                proxy and origin == network_guard._CONFIGURED_PROXY and origin in network_guard._ALLOWED_PROXY_ORIGINS
            )
            if not public and not permitted:
                raise network_guard.NetworkPolicyError("network destination is not permitted")
            addresses.append(address)
    origin = (host.rstrip(".").lower(), port)
    origin_allowed = origin in network_guard._ALLOWED_ORIGINS or (
        proxy and origin == network_guard._CONFIGURED_PROXY and origin in network_guard._ALLOWED_PROXY_ORIGINS
    )
    for address in addresses:
        if not network_guard._address_is_public(address) and not origin_allowed:
            raise network_guard.NetworkPolicyError("network destination is not permitted")
    return addresses[0], port


def _authority(host: str, port: int) -> str:
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def _proxy_auth(parsed) -> bytes | None:
    if parsed.username is None and parsed.password is None:
        return None
    user = unquote(parsed.username or "")
    password = unquote(parsed.password or "")
    token = base64.b64encode(f"{user}:{password}".encode()).decode("ascii")
    return f"Proxy-Authorization: Basic {token}\r\n".encode("ascii")


def _read_headers(sock: socket.socket, timeout: float) -> tuple[bytes, list[bytes], bytes]:
    deadline = time.monotonic() + timeout
    data = bytearray()
    delimiter = b"\r\n\r\n"
    while True:
        end = data.find(delimiter)
        if end >= 0:
            header_end = end + len(delimiter)
            header = bytes(data[:header_end])
            remainder = bytes(data[header_end:])
            break
        if len(data) > _MAX_HEADER:
            raise OSError("invalid request")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise OSError("request header timeout")
        sock.settimeout(remaining)
        chunk = sock.recv(min(_BUFFER_SIZE, _MAX_HEADER + len(delimiter) + 1 - len(data)))
        if not chunk:
            raise OSError("invalid request")
        data.extend(chunk)
    if len(header) > _MAX_HEADER:
        raise OSError("invalid request")
    lines = header[:-4].split(b"\r\n")
    if any(not line or b"\r" in line or b"\n" in line for line in lines):
        raise OSError("invalid request")
    sock.settimeout(timeout)
    return lines[0] + b"\r\n", [line + b"\r\n" for line in lines[1:]], remainder


def _validated_headers(lines: list[bytes], parsed) -> bytes:
    output = []
    connection_tokens = set()
    for line in lines:
        if not line.endswith(b"\r\n") or b"\n" in line[:-2] or b"\r" in line[:-2]:
            raise OSError("invalid request")
        raw_name, separator, raw_value = line[:-2].partition(b":")
        if not separator or not _TOKEN.fullmatch(raw_name):
            raise OSError("invalid request")
        if any(byte < 32 and byte != 9 or byte == 127 for byte in raw_value):
            raise OSError("invalid request")
        name = raw_name.lower()
        if name == b"connection":
            connection_tokens.update(token.strip().lower() for token in raw_value.split(b","))
        if name in (b"host", b"connection", b"proxy-authorization", b"proxy-connection"):
            continue
        output.append(line)
    output = [line for line in output if line.split(b":", 1)[0].strip().lower() not in connection_tokens]
    host = parsed.hostname.rstrip(".").lower()
    port = parsed.port or (443 if parsed.scheme.lower() == "https" else 80)
    default_port = 443 if parsed.scheme.lower() == "https" else 80
    host_header = f"[{host}]" if ":" in host else host
    if port != default_port:
        host_header += f":{port}"
    output.append(f"Host: {host_header}\r\n".encode("ascii"))
    output.append(b"Connection: close\r\n")
    return b"".join(output) + b"\r\n"


def _parse_connect_target(target: str):
    parsed = network_guard._parse_http_url("http://" + target, "invalid request")
    if parsed.username is not None or parsed.password is not None or parsed.path or parsed.query or parsed.fragment:
        raise OSError("invalid request")
    if parsed.port is None or parsed.port == 0 or not parsed.netloc:
        raise OSError("invalid request")
    return parsed.hostname.rstrip(".").lower(), parsed.port


class _RelayServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
    request_queue_size = _MAX_WORKERS

    def __init__(self, address, handler, proxy_parts, timeout):
        self.proxy_parts = proxy_parts
        self.timeout = timeout
        self.slots = threading.BoundedSemaphore(_MAX_WORKERS)
        self.active_lock = threading.Lock()
        self.active_sockets = set()
        self.active_threads = set()
        self.stopping = False
        super().__init__(address, handler)

    def track_socket(self, sock):
        with self.active_lock:
            if self.stopping:
                sock.close()
                raise OSError("relay is shutting down")
            self.active_sockets.add(sock)

    def untrack_socket(self, sock):
        with self.active_lock:
            self.active_sockets.discard(sock)

    def stop_active(self, join_timeout):
        with self.active_lock:
            self.stopping = True
            sockets = tuple(self.active_sockets)
            threads = tuple(self.active_threads)
        for sock in sockets:
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass
        deadline = time.monotonic() + join_timeout
        for worker in threads:
            if worker is not threading.current_thread():
                worker.join(max(0, deadline - time.monotonic()))

    def process_request(self, request, client_address):
        if not self.slots.acquire(blocking=False):
            network_guard.record_guard_failure()
            request.close()
            return
        try:
            self.track_socket(request)
            super().process_request(request, client_address)
        except BaseException:
            self.untrack_socket(request)
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        with self.active_lock:
            self.active_threads.add(threading.current_thread())
        try:
            super().process_request_thread(request, client_address)
        finally:
            with self.active_lock:
                self.active_threads.discard(threading.current_thread())
            self.untrack_socket(request)
            self.slots.release()


class _RelayHandler(socketserver.BaseRequestHandler):
    def handle(self):
        client = self.request
        client.settimeout(self.server.timeout)
        upstream = None
        try:
            first_line, headers, client_remainder = _read_headers(client, self.server.timeout)
            fields = first_line.decode("latin-1").rstrip("\r\n").split(" ", 2)
            if len(fields) != 3 or fields[2] not in ("HTTP/1.0", "HTTP/1.1"):
                raise OSError("invalid request")
            method, target, version = fields
            if not _TOKEN.fullmatch(method.encode("ascii", "strict")):
                raise OSError("invalid request")
            if method.upper() == "CONNECT":
                host, port = _parse_connect_target(target)
                request_data = None
            else:
                parsed = network_guard._parse_http_url(target, "invalid request")
                if parsed.fragment:
                    raise OSError("invalid request")
                host = parsed.hostname.rstrip(".").lower()
                port = parsed.port or (443 if parsed.scheme.lower() == "https" else 80)
                request_data = (
                    f"{method} {parsed.path or '/'}".encode("latin-1")
                    + (b"?" + parsed.query.encode("latin-1") if parsed.query else b"")
                    + f" {version}\r\n".encode("latin-1")
                    + _validated_headers(headers, parsed)
                )
            numeric_ip, port = _resolve_pinned(host, port)
            upstream = _connect_proxy(self.server.proxy_parts, self.server.timeout, self.server)
            authority = _authority(numeric_ip, port).encode("ascii")
            auth = _proxy_auth(self.server.proxy_parts[0])
            connect_request = b"CONNECT " + authority + b" HTTP/1.1\r\nHost: " + authority + b"\r\n"
            if auth:
                connect_request += auth
            upstream.sendall(connect_request + b"\r\n")
            status, _response_headers, upstream_remainder = _read_headers(upstream, self.server.timeout)
            status_parts = status.rstrip(b"\r\n").split()
            if len(status_parts) < 2 or status_parts[0] not in (b"HTTP/1.0", b"HTTP/1.1") or status_parts[1] != b"200":
                network_guard.record_guard_failure()
                client.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                return
            if request_data is None:
                client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                _relay_bytes(client, upstream, self.server.timeout, client_remainder, upstream_remainder)
            else:
                upstream.sendall(request_data + client_remainder)
                _relay_bytes(client, upstream, self.server.timeout, b"", upstream_remainder)
        except (OSError, ValueError, UnicodeError, ssl.SSLError):
            network_guard.record_guard_failure()
            try:
                client.sendall(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            except OSError:
                pass
        finally:
            if upstream is not None:
                self.server.untrack_socket(upstream)
                try:
                    upstream.close()
                except OSError:
                    pass


def _connect_proxy(parts, timeout: float, server=None):
    parsed, proxy_host, proxy_port = parts
    proxy_ip, _ = _resolve_pinned(proxy_host, proxy_port, proxy=True)
    family = socket.AF_INET6 if ipaddress.ip_address(proxy_ip).version == 6 else socket.AF_INET
    upstream = socket.socket(family, socket.SOCK_STREAM)
    upstream.settimeout(timeout)
    if server is not None:
        server.track_socket(upstream)
    raw_upstream = None
    try:
        address = (proxy_ip, proxy_port, 0, 0) if family == socket.AF_INET6 else (proxy_ip, proxy_port)
        upstream.connect(address)
        if parsed.scheme.lower() == "https":
            context = ssl.create_default_context()
            raw_upstream = upstream
            wrapped = context.wrap_socket(raw_upstream, server_hostname=proxy_host, do_handshake_on_connect=False)
            upstream = wrapped
            if server is not None:
                server.track_socket(wrapped)
                server.untrack_socket(raw_upstream)
            upstream.do_handshake()
        return upstream
    except BaseException:
        if server is not None:
            server.untrack_socket(upstream)
            if raw_upstream is not None:
                server.untrack_socket(raw_upstream)
        upstream.close()
        raise


def _relay_bytes(
    left: socket.socket,
    right: socket.socket,
    timeout: float,
    left_remainder: bytes = b"",
    right_remainder: bytes = b"",
) -> None:
    if left_remainder:
        right.sendall(left_remainder)
    if right_remainder:
        left.sendall(right_remainder)
    peers = (left, right)
    while True:
        pending = [peer for peer in peers if isinstance(peer, ssl.SSLSocket) and peer.pending()]
        readable, _, _ = select.select(peers, (), (), 0 if pending else timeout)
        for source in pending:
            if source not in readable:
                readable.append(source)
        if not readable:
            return
        for source in readable:
            data = source.recv(_BUFFER_SIZE)
            if not data:
                return
            target = right if source is left else left
            target.sendall(data)


@contextmanager
def proxy_route(proxy_url: str | None, timeout_secs: float):
    """Yield a local relay URL that pins each destination before CONNECT, or None."""
    if proxy_url is None:
        yield None
        return
    if timeout_secs <= 0:
        raise ValueError("invalid proxy timeout")
    parsed = network_guard._parse_http_url(proxy_url, "invalid proxy", allow_credentials=True)
    proxy_host, proxy_port = network_guard._host_port(parsed)
    if network_guard._CONFIGURED_PROXY != (proxy_host, proxy_port):
        raise network_guard.NetworkPolicyError("network destination is not permitted")
    _resolve_pinned(proxy_host, proxy_port, proxy=True)
    server = _RelayServer(("127.0.0.1", 0), _RelayHandler, (parsed, proxy_host, proxy_port), timeout_secs)
    thread = threading.Thread(target=server.serve_forever, name="worker-proxy-relay", daemon=True)
    host, port = server.server_address
    with _active_lock:
        previous = network_guard._ACTIVE_RELAY_ORIGIN
        network_guard._ACTIVE_RELAY_ORIGIN = (host, port)
    thread.start()
    try:
        yield f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.stop_active(timeout_secs)
        server.server_close()
        thread.join(timeout=timeout_secs)
        with _active_lock:
            network_guard._ACTIVE_RELAY_ORIGIN = previous
