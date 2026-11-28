"""Exercise the native stream ABI and SigMF publication with a software receiver."""
import datetime
import json
import os
from pathlib import Path
import shlex
import struct
import subprocess
import sys
import tempfile

binary = Path(sys.argv[1]).resolve()
source = Path(__file__).with_name("soapy_fixture.cpp")
with tempfile.TemporaryDirectory() as work:
    root = Path(work)
    flags = shlex.split(subprocess.check_output(["pkg-config", "--cflags", "--libs", "SoapySDR"], text=True))
    subprocess.run(["c++", "-std=c++17", "-shared", "-fPIC", str(source), "-o", str(root / "libfixture.so"), *flags], check=True)
    env = dict(os.environ, SOAPY_SDR_PLUGIN_PATH=work)
    for fault in ["", "unlocked", "untimed", "gap"]:
        out = root / ("valid" if not fault else fault)
        settings = {"device": "driver=meridian_fixture,fault=" + fault, "channel": 0,
                    "position": {"lat": 0, "lon": 0}, "receiver_id": "fixture",
                    "time_sigma_ns": 100, "sample_rate_hz": 48000, "frequency_hz": 648000, "duration_s": 0.1}
        config = root / "settings.json"
        config.write_text(json.dumps(settings))
        run = subprocess.run([binary, "capture", config, out], env=env, capture_output=True, text=True, timeout=10)
        if fault:
            assert run.returncode and not list(out.iterdir()), (fault, run.stderr, list(out.iterdir()))
            continue
        assert run.returncode == 0, run.stderr
        meta = Path(run.stdout.strip())
        doc = json.loads(meta.read_text())
        assert doc["global"]["core:sample_rate"] == 48000
        anchor = doc["captures"][0]
        assert anchor["core:frequency"] == 648000
        utc = datetime.datetime.fromisoformat(anchor["core:datetime"].replace("Z","+00:00")).timestamp()
        assert 1700000001 <= utc < 1700000010, utc
        assert meta.with_suffix(".sigmf-data").read_bytes() == struct.pack("<ff", 0.25, -0.5) * 4800
print("SoapySDR fixture: GPS/PPS epoch, IQ and metadata verified; unlocked, untimed and discontinuous streams rejected")
