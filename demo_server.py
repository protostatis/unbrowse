#!/usr/bin/env python3
import http.server, urllib.request, json, os, sys

HOST, PORT = '0.0.0.0', 9090
MCP_TARGET = 'https://unchainedsky.com/unbrowser-mcp'

class H(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        return super().do_GET()
    def do_POST(self):
        if self.path != '/unbrowser-mcp': return self.send_error(404)
        clen = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(clen) if clen else b''
        uheaders = {'Content-Type':'application/json','Accept':'application/json, text/event-stream'}
        incoming_sid = self.headers.get('mcp-session-id','')
        if incoming_sid: uheaders['mcp-session-id'] = incoming_sid
        try:
            req = urllib.request.Request(MCP_TARGET, data=body, headers=uheaders, method='POST')
            with urllib.request.urlopen(req) as resp:
                rbody = resp.read()
                sid = resp.headers.get('mcp-session-id')
                self.send_response(200)
                if sid: self.send_header('mcp-session-id', sid)
                self.send_header('Content-Type','application/json')
                self.send_header('Access-Control-Allow-Origin','*')
                self.end_headers()
                self.wfile.write(rbody)
        except urllib.error.HTTPError as e:
            self.send_response(e.code)
            self.send_header('Content-Type','application/json')
            self.send_header('Access-Control-Allow-Origin','*')
            self.end_headers()
            self.wfile.write(e.read())
        except Exception as e:
            self.send_error(502, f'Proxy error: {e}')
    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header('Access-Control-Allow-Origin','*')
        self.send_header('Access-Control-Allow-Methods','GET,POST,OPTIONS')
        self.send_header('Access-Control-Allow-Headers','Content-Type,Accept,mcp-session-id')
        self.send_header('Access-Control-Expose-Headers','mcp-session-id')
        self.end_headers()

if __name__ == '__main__':
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    http.server.HTTPServer((HOST, PORT), H).serve_forever()
