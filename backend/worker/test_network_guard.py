"""Focused regression tests for worker HTTP destination guarding."""
from __future__ import annotations

import socket
import unittest
from unittest import mock
from urllib.request import OpenerDirector, Request

import network_guard


def _result(address: str):
    family = socket.AF_INET6 if ":" in address else socket.AF_INET
    sockaddr = (address, 80, 0, 0) if family == socket.AF_INET6 else (address, 80)
    return family, socket.SOCK_STREAM, 6, "", sockaddr


class NetworkGuardTests(unittest.TestCase):
    def tearDown(self):
        network_guard.restore_guard()

    def test_mixed_and_ipv4_mapped_answers_are_rejected(self):
        network_guard.install_guard(())
        for address in ("127.0.0.1", "::ffff:127.0.0.1"):
            with self.subTest(address=address), mock.patch.object(
                network_guard, "_ORIGINAL_GETADDRINFO",
                return_value=[_result("8.8.8.8"), _result(address)],
            ), self.assertRaises(OSError):
                network_guard.guarded_getaddrinfo("mixed.test", 80)

    def test_private_redirect_target_is_rejected_before_transport(self):
        network_guard.install_guard(("https://source.test",))
        opener = OpenerDirector()

        def emulate_redirect(_opener, _url, _data, _timeout):
            return network_guard._guarded_opener_open(
                opener, "http://redirect.test/private"
            )

        with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", side_effect=[
            [_result("8.8.8.8")], [_result("127.0.0.1")],
        ]), mock.patch.object(network_guard, "_ORIGINAL_OPENER_OPEN", side_effect=emulate_redirect):
            with self.assertRaises(OSError):
                opener.open("http://source.test/start")

    def test_proxy_allowlist_never_authorizes_a_source_url(self):
        network_guard.install_guard((), ("http://proxy.test:8080",))
        private = [_result("127.0.0.1")]
        with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=private):
            with self.assertRaises(OSError):
                network_guard.validate_source_url("http://proxy.test:8080/path", ())
            network_guard.configure_proxy("http://proxy.test:8080")
            self.assertEqual(network_guard.guarded_getaddrinfo("proxy.test", 8080), private)

    def test_source_allowlist_requires_exact_host_and_port(self):
        network_guard.install_guard(("http://fixture.test:8123/",))
        private = [_result("127.0.0.1")]
        with mock.patch.object(network_guard, "_ORIGINAL_GETADDRINFO", return_value=private):
            network_guard.validate_source_url("http://fixture.test:8123/media", ("http://fixture.test:8123",))
            with self.assertRaises(OSError):
                network_guard.validate_source_url("http://fixture.test:8124/media", ("http://fixture.test:8123",))
            with self.assertRaises(OSError):
                network_guard.guarded_getaddrinfo("fixture.test", 8124)

    def test_url_validation_rejects_controls_credentials_and_non_http(self):
        for url in (
            "http://user:secret@example.test/media",
            "file:///etc/passwd",
            "http://example.test/\nheader",
        ):
            with self.subTest(url=url), self.assertRaises(ValueError) as caught:
                network_guard.validate_source_url(url, ())
            self.assertNotIn("secret", str(caught.exception))

    def test_proxy_validation_and_error_messages_do_not_expose_credentials(self):
        network_guard.install_guard((), ("http://proxy.test:8080",))
        network_guard.configure_proxy("http://user:secret@proxy.test:8080")
        self.assertEqual(network_guard._CONFIGURED_PROXY, ("proxy.test", 8080))
        for url in ("ftp://proxy.test", "http://user:secret@proxy.test:invalid"):
            with self.subTest(url=url), self.assertRaises(ValueError) as caught:
                network_guard.configure_proxy(url)
            self.assertNotIn("secret", str(caught.exception))
        with self.assertRaises(ValueError):
            network_guard.configure_proxy("http://127.0.0.1:3128")

    def test_nested_install_and_restore_do_not_leak_global_methods(self):
        original = socket.getaddrinfo
        with network_guard.guard_context(("https://outer.test",)):
            outer_origins = network_guard._ALLOWED_ORIGINS
            network_guard.install_guard(("https://inner.test",))
            self.assertEqual(network_guard._ALLOWED_ORIGINS, frozenset({("inner.test", 443)}))
            network_guard.install_guard(("https://inner.test",))
            self.assertIs(socket.getaddrinfo, network_guard.guarded_getaddrinfo)
            self.assertNotEqual(outer_origins, network_guard._ALLOWED_ORIGINS)
        self.assertIs(socket.getaddrinfo, original)
        self.assertEqual(network_guard._ALLOWED_ORIGINS, frozenset())

    def test_opener_and_requests_check_each_outbound_request(self):
        network_guard.install_guard(())
        opener = OpenerDirector()
        with self.assertRaises(ValueError):
            opener.open(Request("ftp://example.test/file"))
        try:
            import requests
        except ImportError:
            return
        session = requests.Session()
        request = requests.Request("GET", "file:///etc/passwd").prepare()
        with self.assertRaises(ValueError):
            session.send(request, proxies={})


if __name__ == "__main__":
    unittest.main()
