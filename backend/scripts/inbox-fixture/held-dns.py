"""Disposable DNS/control service. Executed ONLY inside an owned helper container.

No forwarding, address answers, HTTP content, host mounts or outgoing connections.
The default Docker bridge supplies a unique namespace/IP; nothing is published.
"""
import hmac
import http.server
import json
import socket
import socketserver
import struct
import sys
import threading

HOSTNAME, TOKEN = sys.argv[1:]
assert HOSTNAME.endswith('.example.com') and len(HOSTNAME) < 100
assert len(TOKEN) == 48
lock = threading.Lock()
released = threading.Event()
records = []
pending = []


def question(packet):
    if not 12 <= len(packet) <= 4096:
        raise ValueError('invalid packet size')
    ident, flags, count, _, _, _ = struct.unpack('!6H', packet[:12])
    if flags & 0xf800 or count != 1:
        raise ValueError('invalid query header')
    labels, offset = [], 12
    while True:
        if offset >= len(packet):
            raise ValueError('unterminated name')
        length = packet[offset]
        offset += 1
        if length == 0:
            break
        if length > 63 or offset + length > len(packet):
            raise ValueError('invalid label')
        labels.append(packet[offset:offset + length].decode('ascii'))
        offset += length
        if len(labels) > 32:
            raise ValueError('too many labels')
    if offset + 4 > len(packet):
        raise ValueError('missing question fields')
    kind, cls = struct.unpack('!2H', packet[offset:offset + 4])
    if cls != 1:
        raise ValueError('unsupported query class')
    return ident, flags, '.'.join(labels).lower(), kind, packet[12:offset + 4]


def answer(packet):
    ident, flags, _, _, raw = question(packet)
    # Authoritative NXDOMAIN, zero address records: even a transport-policy
    # regression cannot dial any public/private HTTP destination via this DNS.
    return struct.pack('!6H', ident, 0x8403 | (flags & 0x0100), 1, 0, 0, 0) + raw


def receive(packet, client, transport, deliver):
    try:
        _, _, name, kind, _ = question(packet)
    except (ValueError, UnicodeError, struct.error):
        return
    with lock:
        # Unknown names are authoritative NXDOMAIN, never forwarded. After
        # release the owned name is NXDOMAIN too: genuine resolution denial.
        if name != HOSTNAME:
            deliver(answer(packet))
            return
        if len(records) >= 64:
            return  # bounded and still held, never escapes to public DNS
        record = {'name': name, 'type': kind, 'source': client[0], 'port': client[1], 'transport': transport, 'held': not released.is_set(), 'replied': False, 'rcode': 3}
        records.append(record)
        if released.is_set():
            deliver(answer(packet))
            record['replied'] = True
        else:
            pending.append((packet, deliver, record))


class UDP(socketserver.BaseRequestHandler):
    def handle(self):
        packet, sock = self.request
        receive(packet, self.client_address, 'udp', lambda response: sock.sendto(response, self.client_address))


class TCP(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(1)
        def read_exact(size):
            data = b''
            while len(data) < size:
                chunk = self.request.recv(size - len(data))
                if not chunk:
                    raise EOFError()
                data += chunk
            return data
        try:
            size = struct.unpack('!H', read_exact(2))[0]
            if not 12 <= size <= 4096:
                return
            packet = read_exact(size)
            done = threading.Event()
            def deliver(response):
                self.request.sendall(struct.pack('!H', len(response)) + response)
                done.set()
            receive(packet, self.client_address, 'tcp', deliver)
            while not done.wait(0.1):
                try:
                    if self.request.recv(1, socket.MSG_PEEK | socket.MSG_DONTWAIT) == b'':
                        return
                except (BlockingIOError, TimeoutError):
                    pass
        except (OSError, EOFError, struct.error):
            return


class TCPServer(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    # No address reuse: a collision must fail instead of stealing a listener.
    allow_reuse_address = False


class Control(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.0'
    def setup(self):
        super().setup()
        self.connection.settimeout(2)
    def log_message(self, *_):
        pass
    def handle_request(self):
        self.connection.settimeout(2)
        if not hmac.compare_digest(self.headers.get('X-Fixture-Token', ''), TOKEN):
            self.send_error(403)
            return
        if self.path not in ('/state', '/release') or (self.path == '/release' and self.command != 'POST') or (self.path == '/state' and self.command != 'GET'):
            self.send_error(404)
            return
        if self.headers.get('Transfer-Encoding') or self.headers.get('Content-Length', '0') != '0':
            self.send_error(400)
            return
        with lock:
            if self.path == '/release':
                released.set()
                for packet, deliver, record in pending:
                    try:
                        deliver(answer(packet))
                        record['replied'] = True
                    except OSError:
                        pass  # original owning shutdown closed the old socket
                    record['held'] = False
                pending.clear()
            payload = json.dumps({'phase': 'denying' if released.is_set() else 'holding', 'records': records}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
    do_GET = handle_request
    do_POST = handle_request


udp = socketserver.UDPServer(('0.0.0.0', 53), UDP)
tcp = TCPServer(('0.0.0.0', 53), TCP)
for server in (udp, tcp):
    threading.Thread(target=server.serve_forever, daemon=True).start()
control = http.server.HTTPServer(('0.0.0.0', 8053), Control)
control.serve_forever()
