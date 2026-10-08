"""Container-only bounded transport to the existing native fixture controller.
Not content extraction; never executed on the host or mounted into the image.
"""
import socket
import sys

payload = sys.stdin.buffer.readline(1048577)
if len(payload) > 1048576 or not payload.endswith(b'\n'):
    raise SystemExit('invalid fixture payload')
with socket.create_connection(('127.0.0.1', int(sys.argv[1])), timeout=7) as client:
    client.sendall(payload)
    client.shutdown(socket.SHUT_WR)
    output = bytearray()
    while b'\n' not in output:
        chunk = client.recv(65536)
        if not chunk:
            raise SystemExit('fixture response ended')
        output.extend(chunk)
        if len(output) > 1048576:
            raise SystemExit('fixture response bound')
    sys.stdout.buffer.write(output[:output.index(b'\n') + 1])
