"""Container-only OAuth loopback transport; never a content/provider executable.
The native OAuth API permits HTTP only on loopback. Forward only these three
paths to the owned, literal Docker gateway operator service. No redirects/DNS.
"""
import http.client
import http.server
import sys

host, upstream_port, port = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass  # Never log authorization or device responses.
    def do_POST(self):
        self.forward()
    def do_GET(self):
        self.forward()
    def forward(self):
        if (self.command, self.path) not in {
            ('POST', '/login/device/code'), ('POST', '/login/oauth/access_token'),
            ('GET', '/user'),
        }:
            self.send_error(403)
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 <= length <= 32768:
                raise ValueError()
            body = self.rfile.read(length)
            connection = http.client.HTTPConnection(host, upstream_port, timeout=25)
            headers = {'Content-Type': 'application/json', 'Accept': 'application/json'}
            if self.path == '/user':
                headers['Authorization'] = self.headers.get('Authorization', '')
            connection.request(self.command, self.path, body, headers)
            response = connection.getresponse()
            payload = response.read(32769)
            connection.close()
            if len(payload) > 32768:
                raise ValueError()
            self.send_response(response.status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except Exception:
            self.send_error(502, 'operator relay unavailable')
http.server.ThreadingHTTPServer(('127.0.0.1', port), Handler).serve_forever()
