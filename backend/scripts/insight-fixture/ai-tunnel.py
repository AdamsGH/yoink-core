"""IP-only SOCKS5 relay restricted to exact owned fixture endpoints."""
import ipaddress
import json
import select
import socket
import socketserver
import sys
import threading
import time

port = int(sys.argv[1])
targets = json.loads(sys.argv[2])
slots = threading.BoundedSemaphore(16)

def read_exact(socket, size):
    data = bytearray()
    while len(data) < size:
        part = socket.recv(size - len(data))
        if not part:
            raise EOFError()
        data.extend(part)
    return bytes(data)

class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        if not slots.acquire(blocking=False):
            return
        peer = self.request
        try:
            peer.settimeout(5)
            version, count = read_exact(peer, 2)
            if version != 5 or not 1 <= count <= 8 or 0 not in read_exact(peer, count):
                return
            peer.sendall(b'\x05\x00')
            version, command, reserved, kind = read_exact(peer, 4)
            if (version, command, reserved) != (5, 1, 0) or kind not in (1, 4):
                return
            ip = str(ipaddress.ip_address(read_exact(peer, 4 if kind == 1 else 16)))
            destination_port = int.from_bytes(read_exact(peer, 2), 'big')
            if destination_port not in targets.get(ip, []):
                return
            with socket.create_connection((ip, destination_port), 5) as target:
                peer.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00')
                print(json.dumps({'kind': 'ai-tunnel', 'ip': ip, 'port': destination_port}), flush=True)
                deadline = time.monotonic() + 60
                remaining = 8 * 1024 * 1024
                while time.monotonic() < deadline and remaining:
                    ready, _, _ = select.select([peer, target], [], [], 1)
                    for source in ready:
                        data = source.recv(min(65536, remaining))
                        if not data:
                            return
                        (target if source is peer else peer).sendall(data)
                        remaining -= len(data)
        except (OSError, EOFError, ValueError):
            pass
        finally:
            slots.release()

class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True

Server(('127.0.0.1', port), Handler).serve_forever()
