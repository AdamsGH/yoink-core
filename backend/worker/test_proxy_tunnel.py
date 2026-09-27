"""Regression tests for DNS-pinned operator-proxy relaying."""
from __future__ import annotations

import socket
import threading
import time
import unittest
from unittest import mock

import network_guard
import proxy_tunnel


def _answer(address: str, port: int):
    family = socket.AF_INET6 if ":" in address else socket.AF_INET
    sockaddr = (address, port, 0, 0) if family == socket.AF_INET6 else (address, port)
    return family, socket.SOCK_STREAM, 6, "", sockaddr


class _FakeProxy:

    def __init__(self):
        self.server = socket.socket()
        self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.server.settimeout(0.2)
        self.port = self.server.getsockname()[1]
        self.connect_line = None
        self.auth = None
        self.http_request = None
        self.http_headers = []
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self):
        try:
            client, _ = self.server.accept()
        except socket.timeout:
            return
        except OSError:
            return
        with client:
            with client.makefile("rb", buffering=0) as file:
                self.connect_line = file.readline()
                if not self.connect_line:
                    return
                lines = []
                while True:
                    line = file.readline()
                    if not line:
                        return
                    if line == b"\r\n":
                        break
                    lines.append(line)
                self.auth = next((line for line in lines if line.lower().startswith(b"proxy-authorization:")), None)
                client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                self.http_request = file.readline()
                if not self.http_request:
                    return
                while True:
                    line = file.readline()
                    if not line or line == b"\r\n":
                        break
                    self.http_headers.append(line)
                client.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        self.server.close()

    def close(self):
        self.server.close()
        self.thread.join(timeout=2)


class _ScriptedProxy:
    """One-connection proxy with bounded reads and caller-controlled tunnel behavior."""

    def __init__(self, response: bytes, tunnel_handler=None):
        self.server = socket.socket()
        self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.server.settimeout(0.2)
        self.port = self.server.getsockname()[1]
        self.response = response
        self.tunnel_handler = tunnel_handler
        self.error = None
        self.stopping = threading.Event()
        self.connections = 0
        self.connect_request = b""
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()
    @staticmethod
    def _read_header(sock):
        sock.settimeout(1)
        data = bytearray()
        while b"\r\n\r\n" not in data and len(data) <= proxy_tunnel._MAX_HEADER:
            chunk = sock.recv(min(4096, proxy_tunnel._MAX_HEADER + 1 - len(data)))
            if not chunk:
                break
            data.extend(chunk)
        return bytes(data)

    def _serve(self):
        deadline = time.monotonic() + 5
        try:
            while not self.stopping.is_set() and time.monotonic() < deadline:
                try:
                    client, _ = self.server.accept()
                    break
                except socket.timeout:
                    continue
                except OSError:
                    if self.stopping.is_set():
                        return
                    raise
            else:
                return
            self.connections += 1
            with client:
                self.connect_request = self._read_header(client)
                if b"\r\n\r\n" not in self.connect_request:
                    raise AssertionError("proxy received incomplete CONNECT headers")
                client.sendall(self.response)
                if self.tunnel_handler is not None and self.response.startswith(b"HTTP/1.1 200 "):
                    self.tunnel_handler(client)
        except BaseException as error:
            self.error = error
        finally:
            self.server.close()

    def close(self):
        self.stopping.set()
        self.server.close()
        self.thread.join(timeout=2)
        if self.thread.is_alive():
            raise AssertionError("scripted proxy thread did not stop")
        if self.error is not None:
            raise AssertionError("scripted proxy failed") from self.error


def _receive_exact(sock, size):
    sock.settimeout(1)
    received = bytearray()
    while len(received) < size:
        chunk = sock.recv(size - len(received))
        if not chunk:
            break
        received.extend(chunk)
    return bytes(received)


class ProxyTunnelTests(unittest.TestCase):

    def tearDown(self):
        network_guard.restore_guard()

    def test_http_destination_uses_numeric_connect_and_original_host(self):
        proxy = _FakeProxy()
        network_guard.install_guard((), (f"http://operator.test:{proxy.port}",))
        network_guard.configure_proxy(f"http://user%40name:p%3Ass%40word@operator.test:{proxy.port}")
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO
        source_lookups = []

        def resolve(host, port, *args, **kwargs):
            if host == "operator.test":
                return [_answer("127.0.0.1", port)]
            if host == "source.test":
                source_lookups.append((host, port))
                return [_answer("93.184.216.34", port)]
            if host in ("127.0.0.1", "::1"):
                return original_getaddrinfo(host, port, *args, **kwargs)
            raise AssertionError("unexpected DNS lookup")
        try:
            import requests
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve), proxy_tunnel.proxy_route(
                f"http://user%40name:p%3Ass%40word@operator.test:{proxy.port}", 2
            ) as relay:
                with self.assertRaises(OSError):
                    requests.get(relay, timeout=1)
                with self.assertRaises(OSError):
                    requests.get(
                        "http://source.test/",
                        proxies={"http": f"http://operator.test:{proxy.port}"},
                        timeout=1,
                    )
                response = requests.get(
                    "http://source.test:8081/video?q=1",
                    headers={"Host": "attacker.test", "Connection": "X-Internal", "X-Internal": "drop"},
                    proxies={"http": relay},
                    timeout=2,
                )
            proxy.thread.join(timeout=2)
            self.assertTrue(source_lookups)
            self.assertTrue(all(host == "source.test" for host, _ in source_lookups))
            self.assertEqual(proxy.connect_line, b"CONNECT 93.184.216.34:8081 HTTP/1.1\r\n")
            self.assertEqual(proxy.auth, b"Proxy-Authorization: Basic dXNlckBuYW1lOnA6c3NAd29yZA==\r\n")
            self.assertEqual(proxy.http_request, b"GET /video?q=1 HTTP/1.1\r\n")
            self.assertIn(b"Host: source.test:8081\r\n", proxy.http_headers)
            self.assertIn(b"Connection: close\r\n", proxy.http_headers)
            self.assertNotIn(b"Host: attacker.test\r\n", proxy.http_headers)
            self.assertNotIn(b"X-Internal: drop\r\n", proxy.http_headers)
            self.assertEqual(response.status_code, 200)
            self.assertEqual(response.text, "ok")
        finally:
            proxy.close()

    def test_redirect_to_loopback_is_rejected_before_second_upstream_contact(self):

        def redirect(client):
            request = _ScriptedProxy._read_header(client)
            self.assertIn(b"GET /start HTTP/1.1\r\n", request)
            client.sendall(
                b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\n"
                b"Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
        proxy = _ScriptedProxy(b"HTTP/1.1 200 Connection Established\r\n\r\n", redirect)
        proxy_url = f"http://operator.test:{proxy.port}"
        network_guard.install_guard((), (proxy_url,))
        network_guard.configure_proxy(proxy_url)
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

        def resolve(host, port, *args, **kwargs):
            if host == "operator.test":
                return [_answer("127.0.0.1", port)]
            if host == "source.test":
                return [_answer("93.184.216.34", port)]
            return original_getaddrinfo(host, port, *args, **kwargs)
        try:
            import requests
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve):
                with proxy_tunnel.proxy_route(proxy_url, 2) as relay:
                    with self.assertRaises(OSError):
                        requests.get(
                            "http://source.test/start",
                            proxies={"http": relay},
                            timeout=2,
                        )
            self.assertEqual(proxy.connections, 1)
            self.assertTrue(proxy.connect_request.startswith(b"CONNECT 93.184.216.34:80 HTTP/1.1\r\n"))
        finally:
            proxy.close()

    def test_private_dns_rebinding_is_rejected_before_upstream_contact(self):
        proxy = _ScriptedProxy(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        proxy_url = f"http://operator.test:{proxy.port}"
        network_guard.install_guard((), (proxy_url,))
        network_guard.configure_proxy(proxy_url)
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO
        source_lookups = 0

        def resolve(host, port, *args, **kwargs):
            nonlocal source_lookups
            if host == "operator.test":
                return [_answer("127.0.0.1", port)]
            if host == "source.test":
                source_lookups += 1
                address = "93.184.216.34" if source_lookups == 1 else "127.0.0.1"
                return [_answer(address, port)]
            return original_getaddrinfo(host, port, *args, **kwargs)
        try:
            import requests
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve):
                with proxy_tunnel.proxy_route(proxy_url, 2) as relay:
                    response = requests.get(
                        "http://source.test/resource",
                        proxies={"http": relay},
                        timeout=2,
                    )
            self.assertEqual(response.status_code, 502)
            self.assertGreaterEqual(source_lookups, 2)
            self.assertEqual(proxy.connections, 0)
        finally:
            proxy.close()

    def test_upstream_non_200_and_malformed_status_return_credential_free_502(self):
        credentials = "user%40name:p%3Ass%40word"
        forbidden = (b"user@name", b"p:ss@word", b"dXNlckBuYW1lOnA6c3NAd29yZA==")
        for upstream_response in (
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n",
            b"HTTP/1.1 2000 Not-Actually-OK\r\nContent-Length: 0\r\n\r\n",
        ):
            with self.subTest(status=upstream_response.split(b"\r", 1)[0]):
                proxy = _ScriptedProxy(upstream_response)
                proxy_url = f"http://{credentials}@operator.test:{proxy.port}"
                network_guard.install_guard((), (f"http://operator.test:{proxy.port}",))
                network_guard.configure_proxy(proxy_url)
                original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

                def resolve(host, port, *args, **kwargs):
                    if host == "operator.test":
                        return [_answer("127.0.0.1", port)]
                    if host == "source.test":
                        return [_answer("93.184.216.34", port)]
                    return original_getaddrinfo(host, port, *args, **kwargs)
                try:
                    import requests
                    with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve):
                        with proxy_tunnel.proxy_route(proxy_url, 2) as relay:
                            response = requests.get(
                                "http://source.test/resource",
                                proxies={"http": relay},
                                timeout=2,
                            )
                    self.assertEqual(response.status_code, 502)
                    exposed_response = response.content + b"".join(
                        f"{name}: {value}".encode() for name, value in response.headers.items()
                    )
                    for secret in forbidden:
                        self.assertNotIn(secret, exposed_response)
                    self.assertIn(b"Proxy-Authorization: Basic", proxy.connect_request)
                finally:
                    proxy.close()

    def test_connect_tunnel_relays_opaque_bytes_in_both_directions(self):
        client_payload = bytes(range(256))
        upstream_payload = b"\x16\x03\x01opaque-tls-record\x00\xff"

        def exchange(client):
            received = _receive_exact(client, len(client_payload))
            if received == client_payload:
                client.sendall(upstream_payload)
        proxy = _ScriptedProxy(b"HTTP/1.1 200 Connection Established\r\n\r\n", exchange)
        proxy_url = f"http://operator.test:{proxy.port}"
        network_guard.install_guard((), (proxy_url,))
        network_guard.configure_proxy(proxy_url)
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

        def resolve(host, port, *args, **kwargs):
            if host == "operator.test":
                return [_answer("127.0.0.1", port)]
            if host == "source.test":
                return [_answer("93.184.216.34", port)]
            return original_getaddrinfo(host, port, *args, **kwargs)
        client = None
        try:
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve):
                with proxy_tunnel.proxy_route(proxy_url, 2) as relay:
                    parsed = network_guard._parse_http_url(relay, "invalid relay")
                    client = socket.create_connection((parsed.hostname, parsed.port), timeout=1)
                    client.settimeout(1)
                    client.sendall(b"CONNECT source.test:443 HTTP/1.1\r\nHost: source.test:443\r\n\r\n")
                    response = bytearray()
                    while b"\r\n\r\n" not in response and len(response) < 1024:
                        response.extend(client.recv(1024 - len(response)))
                    self.assertTrue(bytes(response).startswith(b"HTTP/1.1 200 Connection Established\r\n\r\n"))
                    client.sendall(client_payload)
                    self.assertEqual(_receive_exact(client, len(upstream_payload)), upstream_payload)
            self.assertIn(b"CONNECT 93.184.216.34:443 HTTP/1.1\r\n", proxy.connect_request)
        finally:
            if client is not None:
                client.close()
            proxy.close()

    def test_tls_wrapped_socket_is_tracked_before_handshake_and_cleaned_on_failure_or_stop(self):
        proxy_url = "https://operator.test:443"
        network_guard.install_guard((), (proxy_url,))
        network_guard.configure_proxy(proxy_url)
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

        def resolve(host, port, *args, **kwargs):
            if host == "operator.test":
                return [_answer("93.184.216.34", port)]
            return original_getaddrinfo(host, port, *args, **kwargs)
        parsed = network_guard._parse_http_url(proxy_url, "invalid proxy", allow_credentials=True)
        raw = mock.MagicMock()
        wrapped = mock.MagicMock()
        server = proxy_tunnel._RelayServer(
            ("127.0.0.1", 0), proxy_tunnel._RelayHandler, (parsed, "operator.test", 443), 1
        )
        context = mock.MagicMock()
        context.wrap_socket.return_value = wrapped
        observations = []

        def handshake_failure():
            observations.append(wrapped in server.active_sockets and raw not in server.active_sockets)
            raise OSError("handshake failed")

        def stop_during_handshake():
            observations.append(wrapped in server.active_sockets and raw not in server.active_sockets)
            server.stop_active(0.1)
            raise OSError("socket closed during shutdown")
        try:
            for handshake in (handshake_failure, stop_during_handshake):
                server.stopping = False
                with self.subTest(handshake=handshake.__name__), mock.patch.object(
                    network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve
                ), mock.patch.object(proxy_tunnel.socket, "socket", return_value=raw), mock.patch.object(
                    proxy_tunnel.ssl, "create_default_context", return_value=context
                ):
                    wrapped.do_handshake.side_effect = handshake
                    with self.assertRaises(OSError):
                        proxy_tunnel._connect_proxy((parsed, "operator.test", 443), 1, server)
                    context.wrap_socket.assert_called_with(
                        raw, server_hostname="operator.test", do_handshake_on_connect=False
                    )
                    wrapped.close.assert_called()
                    self.assertNotIn(wrapped, server.active_sockets)
                    self.assertNotIn(raw, server.active_sockets)
            self.assertEqual(observations, [True, True])
        finally:
            server.server_close()

    def test_private_mixed_and_malformed_targets_are_rejected(self):
        network_guard.install_guard(())
        for addresses in (
            [_answer("127.0.0.1", 80)],
            [_answer("93.184.216.34", 80), _answer("127.0.0.1", 80)],
        ):
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=addresses):
                with self.assertRaises(OSError):
                    proxy_tunnel._resolve_pinned("source.test", 80)
        with self.assertRaises(ValueError):
            network_guard.validate_source_url("http://user:secret@source.test/", ())

    def test_connect_authority_is_strict(self):
        for target in ("example.test", "user@example.test:443", "example.test:0", "example.test:443/path",
                       "example.test:443?x=1", "example.test:443#frag", "example.test:bad"):
            with self.subTest(target=target), self.assertRaises((OSError, ValueError)):
                proxy_tunnel._parse_connect_target(target)

    def test_header_reads_chunked_delimiter_and_preserves_coalesced_body(self):
        sender, receiver = socket.socketpair()
        try:
            sender.sendall(b"GET http://source.test/ HTTP/1.1\r\nHost: source.test\r")
            sender.sendall(b"\n\r\nbody")
            line, headers, remainder = proxy_tunnel._read_headers(receiver, 1)
            self.assertEqual(line, b"GET http://source.test/ HTTP/1.1\r\n")
            self.assertEqual(headers, [b"Host: source.test\r\n"])
            self.assertEqual(remainder, b"body")
        finally:
            sender.close()
            receiver.close()

    def test_header_size_is_bounded(self):
        sender, receiver = socket.socketpair()
        try:
            sender.sendall(b"GET / HTTP/1.1\r\nX-Test: " + b"a" * proxy_tunnel._MAX_HEADER)
            with self.assertRaises(OSError):
                proxy_tunnel._read_headers(receiver, 1)
        finally:
            sender.close()
            receiver.close()

    def test_header_deadline_covers_slow_trickle_across_chunks(self):
        sender, receiver = socket.socketpair()

        def trickle():
            try:
                for byte in b"GET / HTTP/1.1\r\n":
                    sender.sendall(bytes((byte,)))
                    time.sleep(0.02)
            except OSError:
                pass
        thread = threading.Thread(target=trickle, daemon=True)
        thread.start()
        try:
            with self.assertRaises(OSError):
                proxy_tunnel._read_headers(receiver, 0.1)
        finally:
            sender.close()
            receiver.close()
            thread.join(timeout=1)

    def test_relay_sends_both_remainders_before_select_loop(self):
        left, left_peer = socket.socketpair()
        right, right_peer = socket.socketpair()
        try:
            proxy_tunnel._relay_bytes(left, right, 0.01, b"client-body", b"upstream-data")
            self.assertEqual(left_peer.recv(64), b"upstream-data")
            self.assertEqual(right_peer.recv(64), b"client-body")
        finally:
            left.close()
            left_peer.close()
            right.close()
            right_peer.close()

    def test_shutdown_closes_client_waiting_on_incomplete_headers(self):
        proxy = _FakeProxy()
        network_guard.install_guard((), (f"http://operator.test:{proxy.port}",))
        network_guard.configure_proxy(f"http://operator.test:{proxy.port}")
        original_getaddrinfo = network_guard._ORIGINAL_GETADDRINFO

        def resolve(host, port, *args, **kwargs):
            if host == "operator.test":
                return [_answer("127.0.0.1", port)]
            return original_getaddrinfo(host, port, *args, **kwargs)
        try:
            with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=resolve):
                with proxy_tunnel.proxy_route(f"http://operator.test:{proxy.port}", 5) as relay:
                    client = socket.create_connection(("127.0.0.1", int(relay.rsplit(":", 1)[1])))
                    client.sendall(b"GET http://source.test/ HTTP/1.1\r\n")
                try:
                    client.settimeout(1)
                    try:
                        self.assertEqual(client.recv(1), b"")
                    except ConnectionResetError:
                        pass  # Closing a socket with unread request bytes may send RST.
                finally:
                    client.close()
        finally:
            proxy.close()

    def test_no_proxy_yields_none(self):
        with proxy_tunnel.proxy_route(None, 1) as route:
            self.assertIsNone(route)


if __name__ == "__main__":
    unittest.main()




