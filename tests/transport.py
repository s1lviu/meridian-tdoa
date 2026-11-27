"""Local transport contract test only: no RF and no public receivers are contacted."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest

class TransportTest(unittest.TestCase):
    def test_no_second_connection_attempt(self):
        with socket.socket() as server, tempfile.TemporaryDirectory() as root:
            server.bind(('127.0.0.1', 0))
            server.listen()
            port = server.getsockname()[1]
            fixture = Path(root) / 'recorder.py'
            fixture.write_text('''import socket,sys
port=int(sys.argv[sys.argv.index('-p')+1])
with socket.create_connection(('127.0.0.1',port),timeout=2): pass
try:
    socket.create_connection(('127.0.0.1',port),timeout=2)
except OSError as e:
    assert 'automatic reconnection refused' in str(e)
else:
    raise AssertionError('second connection was allowed')
''')
            env = dict(os.environ, NO_PROXY='127.0.0.1', no_proxy='127.0.0.1')
            subprocess.run([sys.executable, 'scripts/kiwi_transport.py',str(fixture),'-s','127.0.0.1','-p',str(port)],env=env,check=True,timeout=8)

if __name__ == '__main__': unittest.main()
