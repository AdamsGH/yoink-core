"""Non-forwarding exact TLS endpoints, plus private read-only control evidence."""
import http.server
import json
import ssl
import sys
import threading
import time

address, control, config_json = sys.argv[1:]
config = json.loads(config_json)
hosts = ('byok-success.fixture.invalid', 'www.youtube.com', 'generativelanguage.googleapis.com')
events = []
lock = threading.Lock()

def record(kind, **fields):
    with lock:
        if len(events) >= 4096:
            raise ValueError('evidence bound')
        events.append(dict(kind=kind, **fields))

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *args):
        pass
    def reply(self, status, value, content_type='application/json'):
        body = (json.dumps(value) if content_type == 'application/json' else value).encode()
        self.send_response(status)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True
    def do_CONNECT(self):
        self.reply(403, {})
    def do_GET(self):
        self.handle_exact(None)
    def do_POST(self):
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 131072:
                raise ValueError('body bound')
            self.connection.settimeout(3)
            body = json.loads(self.rfile.read(length))
            self.handle_exact(body)
        except (ValueError, OSError):
            self.reply(400, {})
    def handle_exact(self, body):
        host = self.headers.get('Host', '').removesuffix(':443')
        if host not in hosts or host != getattr(self.connection, 'fixture_sni', None):
            record('denied', reason='host-sni')
            self.reply(403, {})
            return
        if host == hosts[1] and self.command == 'GET' and self.path in config['youtube'] and not self.headers.get('Authorization'):
            item = config['youtube'][self.path]
            record('source', path=self.path)
            self.reply(200, item['body'], item['type'])
            return
        if host == hosts[0] and self.command == 'GET' and self.path == '/article' and not self.headers.get('Authorization'):
            record('source', path=self.path)
            self.reply(200, '<html><body><article>' + config['articleMarker'] + '</article></body></html>', 'text/html')
            return
        if host == hosts[0] and self.command == 'GET' and self.path == '/llms.txt':
            self.reply(404, {})
            return
        if host == hosts[0] and self.headers.get('Authorization') == 'Bearer ' + config['byokKey']:
            if self.command == 'GET' and self.path == '/v1/models':
                record('models')
                self.reply(200, {'data': [{'id': config['model']}]})
                return
            if self.command == 'POST' and self.path == '/v1/chat/completions' and body.get('model') == config['model'] and body.get('stream') is False and isinstance(body.get('messages'), list) and any(marker in json.dumps(body['messages'], ensure_ascii=False) for marker in (config['marker'], config['articleMarker'])):
                record('byok-completion')
                self.reply(200, {'choices': [{'message': {'content': config['result']}}]})
                return
        if host == hosts[2] and self.command == 'POST' and self.path == '/v1beta/models/' + config['geminiModel'] + ':generateContent' and self.headers.get('x-goog-api-key') == config['geminiKey'] and isinstance(body.get('contents'), list) and config['marker'] in json.dumps(body['contents'], ensure_ascii=False):
            record('gemini-completion')
            self.reply(200, {'candidates': [{'content': {'parts': [{'text': config['result']}]}}]})
            return
        record('denied', reason='endpoint-contract')
        self.reply(403, {})

class Control(Handler):
    def do_GET(self):
        if self.path == '/state':
            with lock:
                snapshot = list(events)
            self.reply(200, {'events': snapshot})
        else:
            self.reply(404, {})
    def do_POST(self):
        self.reply(403, {})

contexts = {}
for host in hosts:
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    ctx.load_cert_chain('/secrets/' + host + '.pem', '/secrets/' + host + '.key')
    contexts[host] = ctx

def sni(sock, name, _ctx):
    if name not in contexts:
        record('tls-denied')
        return ssl.ALERT_DESCRIPTION_UNRECOGNIZED_NAME
    sock.context = contexts[name]
    sock.fixture_sni = name

for ctx in contexts.values():
    ctx.set_servername_callback(sni)
class TlsServer(http.server.ThreadingHTTPServer):
    def get_request(self):
        try:
            return super().get_request()
        except ssl.SSLError as error:
            record('tls-error', reason=str(error))
            raise

server = TlsServer((address, 443), Handler)
server.socket = contexts[hosts[0]].wrap_socket(server.socket, server_side=True)
for attempt in range(100):
    try:
        control_server = http.server.ThreadingHTTPServer((control, 8080), Control)
        break
    except OSError:
        if attempt == 99:
            raise
        time.sleep(0.1)
threading.Thread(target=control_server.serve_forever, daemon=True).start()
server.serve_forever()
