#!/usr/bin/env python3
"""
Local dev server for unbrowser demo.
Serves demo.html. POST /unbrowser-mcp spawns local unbrowser binary as subprocess
and pipes JSON-RPC through stdin/stdout. Returns MCP-shaped response.
No init/notify handshake needed -- we forward plain JSON-RPC.
"""
import http.server, json, os, subprocess, shutil, sys, socketserver

HOST, PORT = '0.0.0.0', 9090
BINARY = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'target', 'release', 'unbrowser')

if not os.path.exists(BINARY):
    print(f"Binary not found at {BINARY}. Build with: cargo build --release")
    sys.exit(1)

class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        return super().do_GET()

    def do_POST(self):
        if self.path != '/unbrowser-mcp':
            return self.send_error(404)

        clen = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(clen)
        result = None

        try:
            req = json.loads(body)
            # Map MCP tools/call to plain JSON-RPC
            if req.get('method') == 'tools/call':
                args = req.get('params', {}).get('arguments', {})
                url = args.get('url', '')
                method = req.get('params', {}).get('name', 'navigate')
                rpc_body = {'id': req.get('id', 1), 'method': method, 'params': args}
                if 'url' not in rpc_body.get('params', {}):
                    rpc_body['params'] = rpc_body.get('params', {})
            elif req.get('method') == 'cookies_set':
                rpc_body = {'id': req.get('id', 1), 'method': 'cookies_set', 'params': req.get('params', {})}
            else:
                # For init/notify/other MCP messages, just respond OK
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Access-Control-Allow-Origin', '*')
                self.send_header('mcp-session-id', 'local')
                self.end_headers()
                resp = {
                    'jsonrpc': '2.0',
                    'id': req.get('id'),
                    'result': {
                        'protocolVersion': '2024-11-05',
                        'capabilities': {'tools': {'listChanged': False}},
                        'serverInfo': {'name': 'unbrowser', 'version': 'local'},
                    }
                }
                self.wfile.write(json.dumps(resp).encode())
                return

            # Spawn unbrowser subprocess for this single request
            proc = subprocess.run(
                [BINARY],
                input=json.dumps(rpc_body) + '\n',
                capture_output=True,
                text=True,
                timeout=30,
            )

            if proc.returncode != 0:
                raise Exception(proc.stderr.strip() or f'exit {proc.returncode}')

            # Parse the plain JSON-RPC response
            try:
                rpc_result = json.loads(proc.stdout.strip())
            except json.JSONDecodeError:
                rpc_result = json.loads(proc.stdout.strip().split('\n')[0])

            if 'error' in rpc_result:
                raise Exception(rpc_result['error'].get('message', str(rpc_result['error'])))

            # Wrap in MCP content format
            nav_result = rpc_result.get('result', {})
            result = {
                'jsonrpc': '2.0',
                'id': req.get('id'),
                'result': {
                    'content': [{
                        'type': 'text',
                        'text': json.dumps(nav_result, indent=2)
                    }]
                }
            }

        except subprocess.TimeoutExpired:
            result = {'jsonrpc':'2.0','id':req.get('id'),'error':{'code':-1,'message':'Timeout (30s)'}}
        except Exception as e:
            result = {'jsonrpc':'2.0','id':req.get('id'),'error':{'code':-1,'message':str(e)}}

        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Access-Control-Allow-Origin', '*')
        if result:
            self.send_header('mcp-session-id', 'local')
        self.end_headers()
        self.wfile.write(json.dumps(result).encode())

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET,POST,OPTIONS')
        self.send_header('Access-Control-Allow-Headers', 'Content-Type,Accept,mcp-session-id')
        self.send_header('Access-Control-Expose-Headers', 'mcp-session-id')
        self.end_headers()

class ThreadedServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True

if __name__ == '__main__':
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    srv = ThreadedServer((HOST, PORT), H)
    print(f'Local dev server on http://localhost:{PORT}/demo.html')
    print(f'Using binary: {BINARY}')
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        srv.server_close()
