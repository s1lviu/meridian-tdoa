#!/usr/bin/env python3
"""Post-hoc evaluation only. This module is never imported by the Rust solver."""
import json,math
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
x=json.loads((ROOT/'evidence/replay-result.json').read_text());loss=json.loads((ROOT/'evidence/sync-loss-result.json').read_text())
# PTB maßstäbe 1, page 23: 50°01′03″ N, 09°00′34″ E. Not copied from the solver's upstream example.
published=json.loads((ROOT/'web/dcf77-reference.json').read_text())
reference={key:published[key] for key in ('lat','lon')}
def distance(a,b):
 p,q=map(math.radians,[a['lat'],b['lat']]);dl=math.radians(a['lon']-b['lon'])
 return 6371.0088*2*math.asin(min(1,math.sqrt(math.sin((p-q)/2)**2+math.cos(p)*math.cos(q)*math.sin(dl/2)**2)))
assert loss['status']=='undetermined' and loss['estimate'] is None and not loss['hypotheses']
assert sum(r['synchronized'] for r in loss['receivers'])==2
report={'dataset':x['dataset_id'],'unique':x['estimate'] is not None,'reference':reference,'reference_source':published['source'],'evaluation_distance':'Great-circle distance with mean Earth radius 6371.0088 km. Regions solved on WGS84.','hypotheses':[{'center':h['center'],'error_km':distance(h['center'],reference),'model_radius_km':h['radius_km']} for h in ([x['estimate']] if x['estimate'] else x['hypotheses'])],'sync_loss_result':'Position undetermined; 2/3 synchronized receivers, no region or coordinates published.','limitations':'Three receivers, two global solutions. DCF77 is a time reference, not an amateur beacon. Antenna offsets and delays have no published calibration. Historical IQ does not establish current source availability or synchronization. Live checks are documented in docs/LIVE-SOURCES.md. No unique localization or guaranteed accuracy is claimed.','start_utc':x['start_utc'],'end_utc':x['end_utc'],'pairs':[{k:p[k] for k in ['a','b','delay_us','sigma_us','coherence','windows']} for p in x['pairs']]}
for path in [ROOT/'evidence/validation.json',ROOT/'web/validation.json']:path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(report,ensure_ascii=False,indent=2))
