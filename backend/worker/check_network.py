"""Read-only live DNS and HTTPS check using the worker's network policy."""
import argparse
import socket
from urllib.parse import urlsplit
from urllib.request import ProxyHandler, build_opener

import network_guard as guard
from proxy_tunnel import proxy_route


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('url')
    parser.add_argument('--timeout', type=float, default=15)
    parser.add_argument('--proxy', default='')
    parser.add_argument('--allowed-proxy-origin', action='append', default=[])
    args = parser.parse_args()
    if not 0 < args.timeout <= 300:
        parser.error('timeout must be between 0 and 300 seconds')
    with guard.guard_context((), tuple(args.allowed_proxy_origin), dns_timeout=args.timeout):
        guard.configure_proxy(args.proxy or None)
        guard.validate_source_url(args.url, ())
        parsed = urlsplit(args.url)
        answers = guard.resolve_addresses(parsed.hostname, parsed.port or 443, socket.AF_UNSPEC, socket.SOCK_STREAM)
        with proxy_route(args.proxy or None, args.timeout) as proxy:
            proxies = {'http': proxy, 'https': proxy} if proxy else {}
            with build_opener(ProxyHandler(proxies)).open(args.url, timeout=args.timeout) as response:
                response.read(1024)
                print(f'addresses={len(answers)} http={response.status}')


if __name__ == '__main__':
    main()
