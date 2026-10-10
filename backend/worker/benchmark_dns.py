"""Recover real public addresses from Mihomo benchmark-range DNS answers."""
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
        host = (host.decode('ascii') if isinstance(host, bytes) else host).encode('idna').decode('ascii')
        if host in self.cache:
            return self.cache[host]
        connection = http.client.HTTPSConnection('1.1.1.1', timeout=self.timeout)
        try:
            connection.request('GET', '/dns-query?' + urlencode({'name': host, 'type': 'A'}),
                               headers={'Accept': 'application/dns-json'})
            response = connection.getresponse()
            raw = response.read(65537)
            if response.status != 200 or len(raw) > 65536:
                raise ValueError('DNS response rejected')
            data = json.loads(raw)
            if not isinstance(data, dict) or data.get('Status') != 0:
                raise ValueError('DNS response rejected')
            answers = data.get('Answer', [])
            if not isinstance(answers, list) or len(answers) > 32 or any(not isinstance(a, dict) for a in answers):
                raise ValueError('DNS response rejected')
            addresses = tuple(a['data'] for a in answers if a.get('type') == 1)
            if not addresses or len(addresses) > 32:
                raise ValueError('DNS response rejected')
            from network_guard import _address_is_public
            if not all(_address_is_public(address) for address in addresses):
                raise ValueError('DNS response rejected')
            self.cache[host] = addresses
            return addresses
        finally:
            connection.close()
