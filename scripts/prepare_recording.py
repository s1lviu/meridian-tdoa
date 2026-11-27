#!/usr/bin/env python3
"""Generate a manifest from a pinned upstream recording; never used by the solver for coordinates of the transmitter."""
from pathlib import Path
import hashlib,json,urllib.request
ROOT=Path(__file__).resolve().parents[1]; OUT=ROOT/'data/recordings';OUT.mkdir(parents=True,exist_ok=True)
COMMIT='2bb9dc2ecc2c6ebcc13ed11c7cbeadea0cd5dfcd'
BASE=f'https://raw.githubusercontent.com/hcab14/TDoA/{COMMIT}'
HASHES={'HB9ODP':'87916f71b32a519d2abb55a85294112adfb4452397af0b0ccf12555ab3ac775d','JO51xl':'045c038e7bac3321abe9413d41a8c8d0fa3d60bc7a315a6cdb6a573bc484b702','pa0rdt':'90727bc9020b18088228709bbd3e77d0f876db0adcbf9503eb330e7fbc14fc8c'}
receivers=[]
for name,lat,lon in [('HB9ODP',46.499351,8.798828),('JO51xl',51.466044,11.977189),('pa0rdt',51.500500,3.600690)]:
    filename=f'20200813T065220Z_77500_{name}_iq.wav';p=OUT/filename
    if not p.exists(): urllib.request.urlretrieve(f'{BASE}/iq/{filename}',p)
    assert hashlib.sha256(p.read_bytes()).hexdigest()==HASHES[name], f'Original checksum mismatch: {filename}'
    receivers.append(dict(id=name,label=name,independent_device=name,position=dict(lat=lat,lon=lon),position_uncertainty_m=1000,position_evidence=dict(url=f'https://github.com/hcab14/TDoA/blob/{COMMIT}/gnss_pos/{name}.txt',note='Receiver GNSS coordinates published by the author. Exact antenna position and cable delay are undocumented. The declared 1 km operational allowance is an assumption, not verified accuracy.'),file=filename,sha256=hashlib.sha256(p.read_bytes()).hexdigest()))
manifest=dict(id='dcf77-20200813',title='DCF77 · reference recording',kind='public_time_reference',emission_verified=True,mode='replay',frequency_hz=77500,capture_utc='2020-08-13T06:52:20Z',gps_utc_offset_s=18,provenance=dict(url=f'https://github.com/hcab14/TDoA/tree/{COMMIT}',note='Real IQ published by hcab14/TDoA in proc_tdoa_DCF77.m; GPL-3.0. Recorded in 2020, not live. Public time reference used for commissioning; this is not an amateur-radio beacon dataset.'),identity=dict(callsign='DCF77',evidence=dict(url=f'https://github.com/hcab14/TDoA/blob/{COMMIT}/proc_tdoa_DCF77.m',note='The author explicitly associates these three files with DCF77. Identity comes from dataset provenance, not inferred position or frequency alone.')),receivers=receivers)
(OUT/'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2)+'\n')
print(OUT/'manifest.json')
