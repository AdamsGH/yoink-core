import json
import socket
import unittest
from unittest.mock import patch

import network_guard as guard
from benchmark_dns import Resolver


class BenchmarkDNS(unittest.TestCase):
    def tearDown(self):
        guard.restore_guard()

    def test_fake_ip_recovery_is_pinned_and_rejects_private_answers(self):
        def lookup(host, port, *args, **kwargs):
            address = '198.18.0.121' if host == 'www.youtube.com' else host
            return [(socket.AF_INET, socket.SOCK_STREAM, 6, '', (address, port))]

        for address, allowed in [('142.251.156.4', True), ('127.0.0.1', False), ('198.18.0.7', False)]:
            with patch.object(guard, '_ORIGINAL_GETADDRINFO', side_effect=lookup), \
                    patch('benchmark_dns.http.client.HTTPSConnection') as connection:
                response = connection.return_value.getresponse.return_value
                response.status = 200
                response.read.return_value = json.dumps({'Status': 0, 'Answer': [{'type': 1, 'data': address}]}).encode()
                with guard.guard_context((), dns_timeout=1):
                    if allowed:
                        guard.validate_source_url('https://www.youtube.com/watch?v=a', ())
                        self.assertEqual(guard.resolve_addresses('www.youtube.com', 443)[0][4][0], address)
                        self.assertEqual(connection.call_count, 1)
                    else:
                        with self.assertRaises(guard.NetworkPolicyError):
                            guard.validate_source_url('https://www.youtube.com/watch?v=a', ())
                    with self.assertRaises(guard.NetworkPolicyError):
                        guard.validate_source_url('http://198.18.0.1/', ())
                self.assertIsNone(guard._BENCHMARK_DNS)

    def test_https_failure_and_oversized_dns_fail_closed(self):
        with patch('benchmark_dns.http.client.HTTPSConnection') as connection:
            response = connection.return_value.getresponse.return_value
            response.status = 302
            response.read.return_value = b'{}'
            with self.assertRaises(ValueError):
                Resolver(1).addresses('example.com')
            response.status = 200
            response.read.return_value = b'x' * 65537
            with self.assertRaises(ValueError):
                Resolver(1).addresses('example.com')
