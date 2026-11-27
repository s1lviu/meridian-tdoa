import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
const base=process.env.BASE_URL || 'http://127.0.0.1:3001';
const token=process.env.MERIDIAN_ADMIN_TOKEN || readFileSync('.secrets/runtime.env','utf8').trim().split('=')[1];
const checks=[];
async function call(path,{method='GET',body,admin=false,status=200}={}){
  const r=await fetch(base+path,{method,headers:{'Content-Type':'application/json',...(admin?{Authorization:'Bearer '+token}:{})},...(body?{body:JSON.stringify(body)}:{})});
  const data=await r.json();assert.equal(r.status,status,JSON.stringify(data));return data;
}
assert.equal((await call('/api/ready')).status,'ready');
await call('/api/emissions?bbox=nan,0,2,3',{status:400});
await call('/api/capture',{method:'POST',body:{target_id:'dcf77-validation'},status:401});
await call('/api/directory/refresh',{method:'POST',status:401});
checks.push('readiness, malformed bbox rejection, capture/catalog mutations require token');
const jobs=[];
for(const fault of [null,'pa0rdt']){
 const j=await call('/api/replay',{method:'POST',admin:true,body:{dataset_id:'dcf77-20200813',fault_receiver:fault,force:true}});
 assert.ok(!j.cached);
 let done;
 for(let i=0;i<100;i++){
  done=await call('/api/jobs/'+j.job_id);
  if(done.status==='completed')break;
  assert.equal(done.status,'running',JSON.stringify(done));
  await new Promise(r=>setTimeout(r,200));
 }
 assert.equal(done.status,'completed');
 const e=await call('/api/emissions/'+done.emission_id);
 assert.equal(e.mode,'replay');assert.equal(e.status,'undetermined');assert.equal(e.estimate,null);
 assert.equal(e.hypotheses.length,fault?0:2);
 assert.equal(e.receivers.filter(r=>r.synchronized).length,fault?2:3);
 jobs.push(done);
 checks.push(fault?'release binary processed actual IQ with GNSS loss, no position/candidate region':'release binary processed actual IQ without transmitter coordinates, retaining both hypotheses');
}
const catalog=await call('/api/sources?limit=2');
assert.ok(catalog.total>100 && catalog.api_allowed>0);
assert.equal(catalog.receivers.length,2);
assert.equal(catalog.next_offset,2);
assert.ok(catalog.receivers.every(r=>r.reception_confirmed===false));
assert.equal((await call('/api/info')).latest_live,null);
checks.push('real catalog is paginated and separate from RF reception; no fabricated live emissions');
const report={passed:true,utc:new Date().toISOString(),checks,jobs,live_rf_validated:false,catalog:{total:catalog.total,api_allowed:catalog.api_allowed,candidates:catalog.candidates,fetched_at:catalog.fetched_at}};
writeFileSync('evidence/release-api-tests.json',JSON.stringify(report,null,2));
console.log('PASS',checks.length,'release API checks, including two authenticated real-IQ jobs');
