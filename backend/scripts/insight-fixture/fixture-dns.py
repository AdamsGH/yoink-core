"""Exact authoritative A records; no recursion, upstream or forwarding."""
import json
import socket
import struct
import sys

bind, records_json = sys.argv[1:]
records = json.loads(records_json)
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind((bind, 53))
while True:
    packet, peer = sock.recvfrom(4096)
    try:
        ident, flags, count, answers, authority, additional = struct.unpack('!6H', packet[:12])
        if count != 1 or flags & 0x8000 or answers or authority or additional > 1:
            continue
        offset, labels = 12, []
        while packet[offset]:
            length = packet[offset]
            if length > 63 or offset + length + 1 >= len(packet):
                raise ValueError('label')
            labels.append(packet[offset + 1:offset + 1 + length].decode('ascii'))
            offset += length + 1
        offset += 1
        kind, cls = struct.unpack('!HH', packet[offset:offset + 4])
        end = offset + 4
        if additional:
            if packet[end:end + 1] != b'\x00':
                raise ValueError('EDNS name')
            opt_type, _, _, opt_length = struct.unpack('!HHIH', packet[end + 1:end + 11])
            if opt_type != 41 or end + 11 + opt_length != len(packet):
                raise ValueError('EDNS bound')
        elif end != len(packet):
            raise ValueError('trailing')
        name = '.'.join(labels).lower()
        known = name in records and cls == 1
        answer = b''
        if known and kind == 1:
            answer = b'\xc0\x0c' + struct.pack('!HHIH', 1, 1, 0, 4) + socket.inet_aton(records[name])
        response = struct.pack('!6H', ident, 0x8400 | (flags & 0x0100) | (0 if known else 3), 1, int(bool(answer)), 0, 0)
        sock.sendto(response + packet[12:end] + answer, peer)
    except (ValueError, IndexError, UnicodeError, struct.error):
        continue
