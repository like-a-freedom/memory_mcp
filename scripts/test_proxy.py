#!/usr/bin/env python3
"""Minimal reverse proxy used by the Task 8 proxy-streaming test
(`http_proxy_streaming_proxy_gate`).

Why this exists: the release-evidence script needs an executable
proxy that supports a configurable read timeout so the test can
prove the upstream `/mcp` body stays open for at least the
configured timeout (>= 120 seconds) when the server is killed
mid-call. The script is intentionally tiny (no third-party deps
beyond the Python standard library) so the test can launch it on
any CI runner that already has Python 3.

Usage:
    MEMORY_MCP_TEST_PROXY_BIN=python3 scripts/test_proxy.py \
        --listen 127.0.0.1:0 --upstream http://127.0.0.1:8080 \
        --read-timeout 130

Environment variables (alternative to flags):
    PROXY_LISTEN: 127.0.0.1:0  (default)
    PROXY_UPSTREAM: http://127.0.0.1:8080 (required)
    PROXY_READ_TIMEOUT: 130 (seconds, default 130)
    PROXY_BLOCK_METRICS: 1 (block /metrics with 404; default 0)

The proxy keeps the body of an upstream response streaming by
forwarding chunks as they arrive. It sets `X-Accel-Buffering: no`
and `Cache-Control: no-cache` on responses only when the upstream
omits them, so the test can prove the upstream's SSE headers
survive the proxy hop.
"""

import argparse
import os
import socket
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit


def parse_listen(addr: str) -> tuple[str, int]:
    if ":" not in addr:
        raise SystemExit(f"PROXY_LISTEN must be host:port, got {addr!r}")
    host, _, port = addr.rpartition(":")
    return host, int(port)


def parse_url(url: str) -> tuple[str, str, int]:
    parts = urlsplit(url)
    if not parts.hostname:
        raise SystemExit(f"PROXY_UPSTREAM must include a host, got {url!r}")
    return (
        parts.hostname,
        parts.path or "/",
        parts.port or (443 if parts.scheme == "https" else 80),
    )


class ProxyState:
    def __init__(self, upstream: str, read_timeout: float, block_metrics: bool):
        self.upstream_host, self.upstream_base, self.upstream_port = parse_url(upstream)
        self.read_timeout = read_timeout
        self.block_metrics = block_metrics


HOP_BY_HOP = frozenset(
    {
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
    }
)


def read_line(sock: socket.socket, deadline: float) -> str | None:
    """Read a CRLF-terminated line from the socket. Returns the
    line text without the trailing CRLF, or None if the deadline
    elapses or the peer closes the socket first."""
    buf = bytearray()
    while True:
        if time.monotonic() >= deadline:
            return None
        remaining = max(0.0, deadline - time.monotonic())
        sock.settimeout(remaining if remaining > 0 else 0.001)
        try:
            b = sock.recv(1)
        except socket.timeout:
            return None
        except OSError:
            return None
        if not b:
            return None
        if b == b"\n":
            text = buf.decode("latin-1", "replace")
            return text.rstrip("\r")
        buf.extend(b)
        if len(buf) > 65536:
            return None


def make_handler(state: ProxyState):
    class ProxyHandler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        # Write everything to the wire unbuffered so the streaming
        # forwarder can flush chunk-sized writes one at a time.
        wbufsize = 0

        def log_message(self, fmt, *args):  # silence default access log
            pass

        def do_GET(self):
            self._proxy("GET")

        def do_POST(self):
            self._proxy("POST")

        def do_DELETE(self):
            self._proxy("DELETE")

        def do_PUT(self):
            self._proxy("PUT")

        def _proxy(self, method: str) -> None:
            if state.block_metrics and self.path == "/metrics":
                self.send_response(404)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            try:
                self._forward(method)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def _forward(self, method: str) -> None:
            deadline = time.monotonic() + state.read_timeout
            content_length = int(self.headers.get("content-length", "0") or "0")
            body = self.rfile.read(content_length) if content_length else b""
            target_path = state.upstream_base.rstrip("/") + self.path
            try:
                sock = socket.create_connection(
                    (state.upstream_host, state.upstream_port),
                    timeout=state.read_timeout,
                )
            except OSError:
                self.send_response(502)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            try:
                sock.settimeout(state.read_timeout)
                # Build the upstream request manually. Using a raw
                # socket lets us forward the body verbatim and
                # stream the response back without buffering, so
                # the proxy hop is unbuffered.
                lines = [f"{method} {target_path} HTTP/1.1"]
                original_host = self.headers.get("host", "")
                if original_host:
                    lines.append(f"host: {original_host}")
                else:
                    lines.append(
                        f"host: {state.upstream_host}:{state.upstream_port}"
                    )
                for key, value in self.headers.items():
                    if key.lower() in HOP_BY_HOP:
                        continue
                    if key.lower() == "host":
                        continue
                    lines.append(f"{key}: {value}")
                if body and "content-length" not in {
                    k.lower() for k in self.headers.keys()
                }:
                    lines.append(f"content-length: {len(body)}")
                lines.append("")
                lines.append("")
                head = "\r\n".join(lines).encode("ascii")
                sock.sendall(head)
                if body:
                    sock.sendall(body)

                status_line = read_line(sock, deadline)
                if not status_line:
                    return
                try:
                    http_version, status_code, _ = status_line.split(" ", 2)
                except ValueError:
                    return
                status_code = int(status_code)

                upstream_headers: dict[str, str] = {}
                chunked = False
                content_length_resp: int | None = None
                while True:
                    if time.monotonic() >= deadline:
                        return
                    line = read_line(sock, deadline)
                    if line is None:
                        return
                    if line == "":
                        break
                    if ":" not in line:
                        continue
                    name, _, value = line.partition(":")
                    name = name.strip().lower()
                    value = value.strip()
                    upstream_headers[name] = value
                    if name == "transfer-encoding" and "chunked" in value.lower():
                        chunked = True
                    if name == "content-length":
                        try:
                            content_length_resp = int(value)
                        except ValueError:
                            content_length_resp = None

                proxy_status_line = f"HTTP/1.1 {status_code} {status_reason(status_code)}"
                self.wfile.write(proxy_status_line.encode("ascii") + b"\r\n")
                sent_cl = False
                for name, value in upstream_headers.items():
                    if name in HOP_BY_HOP:
                        continue
                    self.wfile.write(f"{name}: {value}\r\n".encode("ascii"))
                    if name == "content-length":
                        sent_cl = True
                if chunked:
                    self.wfile.write(b"transfer-encoding: chunked\r\n")
                self.wfile.write(b"connection: close\r\n")
                if not sent_cl and not chunked:
                    self.wfile.write(b"x-accel-buffering: no\r\n")
                ct = upstream_headers.get("content-type", "")
                if ct.startswith("text/event-stream"):
                    if "cache-control" not in upstream_headers:
                        self.wfile.write(b"cache-control: no-cache\r\n")
                    if "x-accel-buffering" not in upstream_headers:
                        self.wfile.write(b"x-accel-buffering: no\r\n")
                self.wfile.write(b"\r\n")
                self.wfile.flush()

                # Stream the body.
                if chunked:
                    chunk_count = 0
                    while True:
                        if time.monotonic() >= deadline:
                            return
                        size_line = read_line(sock, deadline)
                        if size_line is None:
                            return
                        # An empty line in chunked encoding is
                        # the CRLF that terminates the previous
                        # chunk body. Skip it; the next line is the
                        # next chunk's size.
                        if size_line == "":
                            continue
                        size = int(size_line.split(";", 1)[0], 16)
                        if size == 0:
                            # End-of-chunks: write the
                            # terminating chunk + empty trailer
                            # line, then drain trailers.
                            self.wfile.write(b"0\r\n\r\n")
                            self.wfile.flush()
                            while True:
                                trailer = read_line(sock, deadline)
                                if trailer is None or trailer == "":
                                    break
                            break
                        remaining = size
                        # Write the chunk size line. The proxy
                        # MUST emit the same hex chunk framing as
                        # the upstream so downstream chunked
                        # parsers (hyper, curl) can decode it.
                        self.wfile.write(f"{size:x}\r\n".encode("ascii"))
                        while remaining > 0:
                            if time.monotonic() >= deadline:
                                return
                            sock.settimeout(
                                max(0.001, deadline - time.monotonic())
                            )
                            try:
                                chunk = sock.recv(min(4096, remaining))
                            except socket.timeout:
                                return
                            except OSError:
                                return
                            if not chunk:
                                return
                            self.wfile.write(chunk)
                            remaining -= len(chunk)
                        self.wfile.write(b"\r\n")
                        self.wfile.flush()
                        chunk_count += 1
                elif content_length_resp is not None:
                    remaining = content_length_resp
                    while remaining > 0:
                        if time.monotonic() >= deadline:
                            return
                        sock.settimeout(max(0.001, deadline - time.monotonic()))
                        try:
                            chunk = sock.recv(min(4096, remaining))
                        except socket.timeout:
                            return
                        except OSError:
                            return
                        if not chunk:
                            return
                        self.wfile.write(chunk)
                        remaining -= len(chunk)
                        self.wfile.flush()
                else:
                    # No content-length and not chunked: read until
                    # the upstream closes. This is the SSE
                    # streaming case the test cares about.
                    while True:
                        if time.monotonic() >= deadline:
                            return
                        sock.settimeout(max(0.001, deadline - time.monotonic()))
                        try:
                            chunk = sock.recv(4096)
                        except socket.timeout:
                            return
                        except OSError:
                            return
                        if not chunk:
                            break
                        self.wfile.write(chunk)
                        self.wfile.flush()
            finally:
                try:
                    sock.close()
                except OSError:
                    pass

    return ProxyHandler


STATUS_REASONS = {
    200: "OK",
    201: "Created",
    202: "Accepted",
    204: "No Content",
    301: "Moved Permanently",
    302: "Found",
    304: "Not Modified",
    400: "Bad Request",
    401: "Unauthorized",
    403: "Forbidden",
    404: "Not Found",
    406: "Not Acceptable",
    409: "Conflict",
    413: "Payload Too Large",
    500: "Internal Server Error",
    502: "Bad Gateway",
    503: "Service Unavailable",
}


def status_reason(code: int) -> str:
    return STATUS_REASONS.get(code, "Unknown")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--listen", default=os.environ.get("PROXY_LISTEN", "127.0.0.1:0")
    )
    parser.add_argument("--upstream", default=os.environ.get("PROXY_UPSTREAM"))
    parser.add_argument(
        "--read-timeout",
        type=float,
        default=float(os.environ.get("PROXY_READ_TIMEOUT", "130")),
    )
    parser.add_argument(
        "--block-metrics",
        action="store_true",
        default=os.environ.get("PROXY_BLOCK_METRICS", "0") == "1",
    )
    args = parser.parse_args(argv)
    if not args.upstream:
        print("PROXY_UPSTREAM is required", file=sys.stderr)
        return 2
    state = ProxyState(args.upstream, args.read_timeout, args.block_metrics)
    handler = make_handler(state)
    host, port = parse_listen(args.listen)
    server = ThreadingHTTPServer((host, port), handler)
    print(
        f"test_proxy bound={server.server_address[0]}:{server.server_address[1]}",
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))