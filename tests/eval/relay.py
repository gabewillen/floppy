#!/usr/bin/env python3
"""A local relay for a live run from a cloud session (docs/eval.md):
`scx` does not use the session's HTTPS proxy, so each backend the bot calls
is named in its env file as a plain-HTTP relay on 127.0.0.1, which forwards
every request to its HTTPS upstream through the proxy (urllib reads
HTTPS_PROXY; SSL_CERT_FILE, or the cloud session's CA bundle, verifies it),
streaming the answer back. The proxy adds the backend's credential; the
relay logs nothing.

Usage: python3 tests/eval/relay.py PORT https://upstream
"""

import os
import ssl
import sys
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CAFILE = os.environ.get("SSL_CERT_FILE") or ("/root/.ccr/ca-bundle.crt" if os.path.exists("/root/.ccr/ca-bundle.crt") else None)
OPENER = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ssl.create_default_context(cafile=CAFILE)))
HOP = ("host", "content-length", "connection", "accept-encoding", "transfer-encoding")


class Forward(BaseHTTPRequestHandler):
    upstream = ""
    protocol_version = "HTTP/1.1"

    def _forward(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else None
        headers = {k: v for k, v in self.headers.items() if k.lower() not in HOP}
        request = urllib.request.Request(self.upstream + self.path, data=body, method=self.command, headers=headers)
        try:
            response = OPENER.open(request, timeout=600)
        except urllib.error.HTTPError as error:
            response = error
        except (urllib.error.URLError, TimeoutError) as error:
            message = f'{{"error":{{"message":"relay: {type(error).__name__}"}}}}'.encode()
            self.send_response(502)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)
            return
        self.send_response(response.status if hasattr(response, "status") else response.code)
        for name, value in response.headers.items():
            if name.lower() not in HOP + ("content-encoding",):
                self.send_header(name, value)
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        while True:
            chunk = response.read1(65536) if hasattr(response, "read1") else response.read(65536)
            if not chunk:
                break
            self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
            self.wfile.flush()
        self.wfile.write(b"0\r\n\r\n")

    do_GET = do_POST = do_PUT = do_DELETE = _forward

    def log_message(self, *_args):
        pass


if __name__ == "__main__":
    Forward.upstream = sys.argv[2].rstrip("/")
    ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Forward).serve_forever()
