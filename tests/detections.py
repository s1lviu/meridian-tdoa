#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""End-to-end check of node detections (mlat-style) against a real server process.

Five simulated nodes with free-running clocks report arrival times of a reference emitter
(known position) once per second and of one unknown target. The server must synchronize the
clocks, locate the target, register a track and push a change event. No RF is involved.

Usage: python3 tests/detections.py [path/to/meridian-tdoa]
"""
import hashlib
import json
import math
import os
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
C = 299792.458  # km/s


def distance(a, b):
    """WGS84 is used by the server; the sphere is accurate to ~0.3% here, well inside the tolerance."""
    p, q = math.radians(a[0]), math.radians(b[0])
    h = math.sin((q - p) / 2) ** 2 + math.cos(p) * math.cos(q) * math.sin(math.radians(b[1] - a[1]) / 2) ** 2
    return 6371.0088 * 2 * math.asin(min(1.0, math.sqrt(h)))


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else shutil.which("meridian-tdoa")
    assert binary, "pass the meridian-tdoa binary path"
    work = tempfile.mkdtemp(prefix="meridian-detections-")
    os.symlink(os.path.join(REPO, "web"), os.path.join(work, "web"))
    nodes = {"n0": (44.40, 26.10), "n1": (44.55, 25.80), "n2": (44.20, 25.90), "n3": (44.45, 26.45), "n4": (44.25, 26.30)}
    tokens = {n: "token-for-" + n + "-" + "x" * 24 for n in nodes}
    config = json.load(open(os.path.join(REPO, "config/app.json")))
    config["nodes"] = [
        {"id": n, "label": "simulated " + n, "position": {"lat": p[0], "lon": p[1]},
         "position_evidence": {"url": "https://example.org/simulated", "note": "simulated node"},
         "token_sha256": hashlib.sha256(tokens[n].encode()).hexdigest()}
        for n, p in nodes.items()
    ]
    reference = (44.43, 26.02)
    config["reference_emitters"] = [{"signal_prefix": "ref-", "position": {"lat": reference[0], "lon": reference[1]},
                                     "evidence": {"url": "https://example.org/simulated", "note": "simulated reference"}}]
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    config["listen"] = f"127.0.0.1:{port}"
    with open(os.path.join(work, "config.json"), "w") as f:
        json.dump(config, f)
    env = dict(os.environ, MERIDIAN_CONFIG="config.json", MERIDIAN_DIRECTORY_OFFLINE="1")
    env.pop("PORT", None)
    server = subprocess.Popen([binary], cwd=work, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    base = f"http://127.0.0.1:{port}"
    try:
        for _ in range(100):
            try:
                urllib.request.urlopen(base + "/api/ready", timeout=1)
                break
            except OSError:
                time.sleep(0.1)
        offsets = {"n0": 0.0, "n1": 0.0031, "n2": -0.0047, "n3": 0.0012, "n4": -0.0020}
        drifts = {"n0": 0.0, "n1": 31e-6, "n2": -12e-6, "n3": 40e-6, "n4": -25e-6}
        rnd = random.Random(4)

        def local(n, t):
            return t + offsets[n] + drifts[n] * t + rnd.gauss(0, 100e-9)

        def post(node, batch):
            body = json.dumps({"detections": batch, "frequency_hz": 145.8e6}).encode()
            req = urllib.request.Request(base + "/api/detections", data=body, method="POST",
                                         headers={"Content-Type": "application/json", "Authorization": "Bearer " + tokens[node]})
            return json.load(urllib.request.urlopen(req, timeout=5))

        # A wrong token is refused.
        try:
            req = urllib.request.Request(base + "/api/detections", data=b'{"detections":[]}', method="POST",
                                         headers={"Content-Type": "application/json", "Authorization": "Bearer nope"})
            urllib.request.urlopen(req, timeout=5)
            raise AssertionError("unknown token accepted")
        except urllib.error.HTTPError as e:
            assert e.code == 401, e.code
        stream = urllib.request.urlopen(base + "/api/stream", timeout=20)
        target = (44.36, 26.20)
        for k in range(30):
            t = 1000.0 + k
            for n, p in nodes.items():
                post(n, [{"node": n, "signal": f"ref-{k}", "local_s": local(n, t + distance(reference, p) / C), "sigma_s": 1e-7}])
        time.sleep(3.5)  # references settle and update the clocks first
        for n, p in nodes.items():
            post(n, [{"node": n, "signal": "target-1", "local_s": local(n, 1030.4 + distance(target, p) / C), "sigma_s": 1e-7}])
        deadline = time.time() + 10
        located = None
        while time.time() < deadline and not located:
            time.sleep(0.5)
            data = json.load(urllib.request.urlopen(base + "/api/emissions?mode=live&compact=true&start=2000-01-01T00:00:00Z&end=2100-01-01T00:00:00Z"))
            located = next((e for e in data["emissions"] if e["id"].endswith("target-1") and e["estimate"]), None)
        if not located:
            print(json.dumps(json.load(urllib.request.urlopen(base + "/api/clocks")), indent=1)[:3000])
            print(json.dumps(data, indent=1)[:3000])
        assert located, "target not located"
        c = located["estimate"]["center"]
        error_km = distance((c["lat"], c["lon"]), target)
        assert error_km < 1.0, error_km
        tracks = json.load(urllib.request.urlopen(base + "/api/tracks"))["tracks"]
        assert tracks, "no track registered"
        clocks = json.load(urllib.request.urlopen(base + "/api/clocks"))["clocks"]
        assert clocks["master"] == "n0" and len(clocks["clocks"]) == 4
        line = b""
        while b"event: change" not in line:
            line = stream.readline()
        print(json.dumps({"target_error_km": round(error_km, 3), "radius_km": round(located["estimate"]["radius_km"], 3),
                          "tracks": len(tracks), "push_event": True, "clock_rejections": clocks["rejected"]}))
    finally:
        server.terminate()
        server.wait(timeout=30)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
