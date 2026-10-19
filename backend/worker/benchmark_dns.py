"""Resolve public addresses over HTTPS after DNS failure or Mihomo fake-IP answers."""
import http.client
import ipaddress
import json
from urllib.parse import urlencode

BENCHMARK = ipaddress.ip_network('198.18.0.0/15')


class Resolver:
    def __init__(self, timeout):
        self.timeout = timeout
        self.cache = {}

    def addresses(self, host):
        from network_guard import NetworkPolicyError, NetworkResolutionError
        host = (host.decode('ascii') if isinstance(host, bytes) else host).encode('idna').decode('ascii')
        if host in self.cache:
            return self.cache[host]
        connection = http.client.HTTPSConnection('1.1.1.1', timeout=self.timeout)
        try:
            connection.request('GET', '/dns-query?' + urlencode({'name': host, 'type': 'A'}),
                               headers={'Accept': 'application/dns-json'})
            response = connection.getresponse()
            raw = response.read(65537)
            if response.status != 200:
                raise NetworkResolutionError('DNS service unavailable')
            if len(raw) > 65536:
                raise NetworkResolutionError('DNS response rejected')
            try:
                data = json.loads(raw)
            except ValueError as exc:
                raise NetworkResolutionError('DNS response rejected') from exc
            if not isinstance(data, dict):
                raise NetworkResolutionError('DNS response rejected')
            if data.get('Status') != 0:
                raise NetworkResolutionError('DNS name unavailable')
            answers = data.get('Answer', [])
            if not isinstance(answers, list) or len(answers) > 32 or any(not isinstance(a, dict) for a in answers):
                raise NetworkResolutionError('DNS response rejected')
            addresses = tuple(a.get('data') for a in answers if a.get('type') == 1)
            if not addresses:
                raise NetworkResolutionError('DNS name unavailable')
            if len(addresses) > 32 or any(not isinstance(address, str) for address in addresses):
                raise NetworkResolutionError('DNS response rejected')
            from network_guard import _address_is_public
            try:
                public = all(_address_is_public(address) for address in addresses)
            except ValueError as exc:
                raise NetworkResolutionError('DNS response rejected') from exc
            if not public:
                raise NetworkPolicyError('network destination is not permitted')
            self.cache[host] = addresses
            return addresses
        finally:
            connection.close()
