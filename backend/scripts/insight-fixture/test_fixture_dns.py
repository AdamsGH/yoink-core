"""Run inside the immutable fixture image with --network none."""
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import time

name = 'byok-success.fixture.invalid'
address = '93.184.1.4'
process = subprocess.Popen([
    sys.executable, '-I', '-B', str(Path(__file__).with_name('fixture-dns.py')),
    '127.0.0.1', json.dumps({name: address}),
])
try:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(0.1)
        for hostname, kind, edns, answers, status in [
            (name, 1, False, 1, 0),
            (name, 1, True, 1, 0),
            (name, 28, True, 0, 0),
            ('unknown.fixture.invalid', 1, True, 0, 3),
        ]:
            question = b''.join(bytes([len(part)]) + part.encode() for part in hostname.split('.'))
            question += b'\x00' + struct.pack('!HH', kind, 1)
            packet = struct.pack('!6H', 123, 0x100, 1, 0, 0, int(edns)) + question
            if edns:
                packet += b'\x00' + struct.pack('!HHIH', 41, 4096, 0, 0)
            deadline = time.monotonic() + 2
            while True:
                assert process.poll() is None, 'DNS process exited'
                sock.sendto(packet, ('127.0.0.1', 53))
                try:
                    response, _ = sock.recvfrom(4096)
                    break
                except TimeoutError:
                    assert time.monotonic() < deadline, 'DNS response deadline'
            ident, flags, count, received, authority, additional = struct.unpack('!6H', response[:12])
            assert (ident, count, received, authority, additional) == (123, 1, answers, 0, 0)
            assert flags & 15 == status
            assert response[12:12 + len(question)] == question
            if answers:
                assert response[-4:] == socket.inet_aton(address)
    print('DNS plain/EDNS A, AAAA and NXDOMAIN passed')
finally:
    process.terminate()
    try:
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=2)
