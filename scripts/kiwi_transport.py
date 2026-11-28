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
import re
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
    proxy = os.environ.get('HTTP_PROXY') or os.environ.get('http_proxy') or os.environ.get('ALL_PROXY') or os.environ.get('all_proxy')

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
    # KiwiClient logs server refusal messages (per-IP time limit, busy, down, bad password) only at
    # debug level. Echo just those, one line each, so Meridian can respect them without verbose logs.
    import logging
    debug = logging.debug

    def refusal_aware_debug(msg, *a, **kw):
        text = str(msg) % a if a else str(msg)
        for key in ('ip_limit', 'too_busy', 'badp', 'down', 'redirect', 'inactivity_timeout'):
            if (' %s:' % key) in text or text.rstrip().endswith(' ' + key):
                rest = text.split(key, 1)[1].strip(': ').split()
                if key == 'badp' and rest[:1] == ['0']:
                    continue
                # Only the leading number (e.g. ip_limit minutes), as the native transport logs it;
                # the IP address the receiver saw and redirect URLs are not recorded.
                detail = re.match(r'\d{0,12}', rest[0]).group() if rest else ''
                sys.stderr.write('MERIDIAN-REFUSAL %s %s\n' % (key, detail))
                sys.stderr.flush()
        return debug(msg, *a, **kw)

    logging.debug = refusal_aware_debug
    sys.argv[0] = recorder
    sys.path.insert(0, str(Path(recorder).parent))
    runpy.run_path(recorder, run_name='__main__')


if __name__ == '__main__':
    main()
