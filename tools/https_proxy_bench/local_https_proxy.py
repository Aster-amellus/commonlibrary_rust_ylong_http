#!/usr/bin/env python3
# Copyright (c) 2026 Huawei Device Co., Ltd.
# Licensed under the Apache License, Version 2.0.

import argparse
import http.server
import json
import select
import socket
import socketserver
import ssl
import threading


def set_tcp_nodelay(sock):
    try:
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    except OSError:
        pass


class OriginHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    response_size = 1024

    def setup(self):
        super().setup()
        set_tcp_nodelay(self.connection)

    def do_GET(self):
        body = b"x" * self.response_size
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "keep-alive")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def log_message(self, fmt, *args):
        return


class ThreadingHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


class ThreadingHTTPSServer(ThreadingHTTPServer):
    def __init__(self, address, handler, context):
        super().__init__(address, handler)
        self.context = context

    def get_request(self):
        sock, addr = self.socket.accept()
        set_tcp_nodelay(sock)
        return self.context.wrap_socket(sock, server_side=True), addr


class HttpsProxyServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address, handler, context, response_size):
        super().__init__(address, handler)
        self.context = context
        self.response_size = response_size

    def get_request(self):
        sock, addr = self.socket.accept()
        set_tcp_nodelay(sock)
        return self.context.wrap_socket(sock, server_side=True), addr


class ProxyHandler(socketserver.BaseRequestHandler):
    def handle(self):
        while True:
            header, rest = self._read_header()
            if not header:
                return
            first, headers = header.split(b"\r\n", 1)
            parts = first.decode("ascii", "replace").split()
            if len(parts) < 3:
                return
            method, target, version = parts[0], parts[1], parts[2]
            if method.upper() == "CONNECT":
                self._connect(target, version)
                return
            self._drain_request_body(headers, rest)
            self._direct_http_response(version)

    def _read_header(self):
        data = bytearray()
        while b"\r\n\r\n" not in data:
            chunk = self.request.recv(4096)
            if not chunk:
                return bytes(data), b""
            data.extend(chunk)
            if len(data) > 65536:
                return b"", b""
        marker = data.index(b"\r\n\r\n") + 4
        return bytes(data[:marker]), bytes(data[marker:])

    def _direct_http_response(self, version):
        body = b"x" * self.server.response_size
        response = (
            f"{version} 200 OK\r\n"
            f"Content-Length: {len(body)}\r\n"
            "Connection: keep-alive\r\n"
            "\r\n"
        ).encode("ascii") + body
        self.request.sendall(response)

    def _drain_request_body(self, headers, rest):
        length = 0
        for line in headers.split(b"\r\n"):
            name, _, value = line.partition(b":")
            if name.lower() == b"content-length":
                try:
                    length = int(value.strip())
                except ValueError:
                    length = 0
                break
        length = max(0, length - len(rest))
        while length > 0:
            data = self.request.recv(min(length, 64 * 1024))
            if not data:
                return
            length -= len(data)

    def _connect(self, target, version):
        host, port = split_host_port(target)
        try:
            upstream = socket.create_connection((host, port), timeout=10)
            set_tcp_nodelay(upstream)
        except OSError:
            self.request.sendall(f"{version} 502 Bad Gateway\r\n\r\n".encode("ascii"))
            return
        self.request.sendall(f"{version} 200 Connection Established\r\n\r\n".encode("ascii"))
        try:
            relay(self.request, upstream)
        finally:
            upstream.close()


def split_host_port(target):
    if target.startswith("["):
        host, _, rest = target[1:].partition("]")
        port = int(rest.removeprefix(":"))
        return host, port
    host, _, port = target.rpartition(":")
    return host, int(port)


def relay(left, right):
    sockets = [left, right]
    while True:
        readable, _, _ = select.select(sockets, [], [], 30)
        if not readable:
            return
        for src in readable:
            data = src.recv(16384)
            if not data:
                return
            dst = right if src is left else left
            dst.sendall(data)


def serve(server):
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return thread


def build_server_context(cert_file, key_file):
    context = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    context.load_cert_chain(cert_file, key_file)
    return context


def build_proxy_context(args):
    context = build_server_context(args.cert_file, args.key_file)
    if args.require_client_cert:
        context.verify_mode = ssl.CERT_REQUIRED
        context.load_verify_locations(cafile=args.ca_file)
    return context


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cert-file", required=True)
    parser.add_argument("--key-file", required=True)
    parser.add_argument("--ca-file")
    parser.add_argument("--require-client-cert", action="store_true")
    parser.add_argument("--origin-tls", action="store_true")
    parser.add_argument("--origin-port", type=int, default=18080)
    parser.add_argument("--proxy-port", type=int, default=18443)
    parser.add_argument("--response-size", type=int, default=1024)
    return parser.parse_args()


def main():
    args = parse_args()
    if args.require_client_cert and not args.ca_file:
        raise SystemExit("--require-client-cert requires --ca-file")

    OriginHandler.response_size = args.response_size
    if args.origin_tls:
        origin = ThreadingHTTPSServer(
            ("127.0.0.1", args.origin_port),
            OriginHandler,
            build_server_context(args.cert_file, args.key_file),
        )
    else:
        origin = ThreadingHTTPServer(("127.0.0.1", args.origin_port), OriginHandler)

    proxy = HttpsProxyServer(
        ("127.0.0.1", args.proxy_port),
        ProxyHandler,
        build_proxy_context(args),
        args.response_size,
    )

    serve(origin)
    serve(proxy)

    origin_host, origin_port = origin.server_address
    _, proxy_port = proxy.server_address
    origin_scheme = "https" if args.origin_tls else "http"
    origin_url = f"{origin_scheme}://{origin_host}:{origin_port}/"
    payload = {
        "origin_url": origin_url,
        "https_proxy": f"https://localhost:{proxy_port}",
    }
    if args.origin_tls:
        payload["https_url"] = origin_url
    else:
        payload["http_url"] = origin_url
    print(json.dumps(payload), flush=True)

    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        origin.shutdown()
        proxy.shutdown()


if __name__ == "__main__":
    main()
