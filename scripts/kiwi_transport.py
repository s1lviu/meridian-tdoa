#!/usr/bin/env python3
"""Run pinned, unmodified KiwiClient through an optional standard HTTP CONNECT proxy.

The wire protocol and client identification remain KiwiClient's. Connections are
restricted to the explicitly selected endpoints; no retry or alternate target is
introduced here. This also rejects unreviewed receiver redirects.
"""
import base64
import http.client
import os
from pathlib import Path
import runpy
import socket
import sys
import threading
import urllib.parse
import urllib.request


def main():
    recorder = str(Path(sys.argv.pop(1)).resolve())
    args = sys.argv[1:]
    hosts = args[args.index('-s') + 1].split(',')
    ports = [int(p) for p in args[args.index('-p') + 1].split(',')]
    if len(ports) == 1:
        ports *= len(hosts)
    allowed = set(zip(hosts, ports))
    attempted = set()
    attempt_lock = threading.Lock()
    original = socket.create_connection
    proxy = os.environ.get('HTTP_PROXY') or os.environ.get('http_proxy')

    def connect(address, timeout=socket._GLOBAL_DEFAULT_TIMEOUT, source_address=None, **kwargs):
        host, port = address
        if (host, port) not in allowed:
            raise OSError('Receiver redirected to an endpoint outside the verified capture group')
        with attempt_lock:
            if (host, port) in attempted:
                raise OSError('One connection attempt per receiver: automatic reconnection refused')
            attempted.add((host, port))
        if not proxy or urllib.request.proxy_bypass(host):
            return original(address, timeout, source_address, **kwargs)
        url = urllib.parse.urlsplit(proxy)
        if url.scheme != 'http' or not url.hostname:
            raise OSError('Recorder supports HTTP CONNECT proxies only')
        tunnel = http.client.HTTPConnection(url.hostname, url.port or 80, timeout=timeout)
        tunnel._create_connection = original
        headers = {}
        if url.username is not None:
            credential = urllib.parse.unquote(url.username) + ':' + urllib.parse.unquote(url.password or '')
            headers['Proxy-Authorization'] = 'Basic ' + base64.b64encode(credential.encode()).decode()
        tunnel.set_tunnel(host, port, headers=headers)
        tunnel.connect()
        return tunnel.sock

    socket.create_connection = connect
    sys.argv[0] = recorder
    sys.path.insert(0, str(Path(recorder).parent))
    runpy.run_path(recorder, run_name='__main__')


if __name__ == '__main__':
    main()
