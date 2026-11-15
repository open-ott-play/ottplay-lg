'use strict';
// Execute the actual controller with fake process, FS, Luna and child pipes.
// No child process, device, clock sleep or network operation is performed.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),Events=require('node:events');
const source=fs.readFileSync(path.join(__dirname,'../app/controller.js'),'utf8'),policy=require('../app/policy');
function fixture(){
 const files=new Map(),requests=[],timers=[],children=[],handlers={},exits=[];let millis=0;
 const root='/fixture/app',shared='/media/developer/apps/usr/palm/services/com.openottplay.homehelper.control/state';
 files.set(root+'/options.json',JSON.stringify({nativeUid:1002,sharedGid:1001}));
 files.set('/proc/sys/kernel/random/boot_id','boot');
 files.set(shared+'/control.json',JSON.stringify({session:'session',boot:'boot',enabled:true,paused:false,mode:'enabled',accepted:true}));
 const fakeFs={readFileSync(p){if(!files.has(p))throw Error('missing');return files.get(p);},writeFileSync(p,v){files.set(p,v);},renameSync(a,b){files.set(b,files.get(a));files.delete(a);},existsSync(p){return files.has(p);},unlinkSync(p){if(!files.delete(p))throw Object.assign(Error('missing'),{code:'ENOENT'});}};
 function child(pid){const c=new Events();c.pid=pid;c.stdout=new Events();c.stdin=new Events();c.stdin.end=v=>{c.ended=v;};c.stdin.write=()=>{};c.kill=()=>{c.killed=true;};return c;}
 const cp={spawn(exe,args){assert.equal(exe,root+'/relay');const c=child(12+children.length);c.args=args;children.push(c);return c;}};
 function Service(){this.call=(uri,p,cb)=>{assert.equal(uri,'luna://com.webos.applicationManager/launch');requests.push({id:p.id,cb});};}
 const proc={argv:['node','controller',JSON.stringify({session:'session',mode:'enabled',trialSeconds:0})],pid:11,getuid:()=>1002,getgid:()=>1001,getgroups:()=>[1001],on:(name,fn)=>{handlers[name]=fn;},exit:code=>{exits.push(code);},hrtime(previous){const current=[Math.floor(millis/1000),(millis%1000)*1e6];if(!previous)return current;let sec=current[0]-previous[0],nano=current[1]-previous[1];if(nano<0){sec--;nano+=1e9;}return [sec,nano];}};
 const ps={read(pid){return {tick:pid===11?'parent':'worker',state:'S'};},matches(p,t){return p.tick===t;}};
 const env={__dirname:root,process:proc,Date:{now:()=>100000+millis},require(n){if(n==='fs')return fakeFs;if(n==='child_process')return cp;if(n==='/usr/lib/node_modules/webos-service')return Service;if(n==='./process-state')return ps;if(n==='./policy')return policy;throw Error(n);},setTimeout(fn,ms){const t={fn,ms};timers.push(t);return t;},clearTimeout(t){if(t)t.cleared=true;},setInterval(fn,ms){return {fn,ms};},clearInterval(){}};
 vm.runInNewContext(source,env);
 const worker=children[0];worker.stdout.emit('data',Buffer.from('READY\n'));
 return {requests,timers,children,handlers,exits,worker,line(value){worker.stdout.emit('data',Buffer.from(value+'\n'));},advance(ms){millis+=ms;},lastTimer(ms){return timers.filter(t=>!t.cleared&&t.ms===ms).at(-1);}};
}
// The actual reported counterexample: OTT request still pending at 2 seconds.
{
 const f=fixture();f.line('HOME');assert.deepEqual(f.requests.map(x=>x.id),['com.openottplay.player.native']);
 f.advance(2000);f.line('LGHOME');assert.equal(f.requests.length,1);
 f.requests[0].cb({payload:{returnValue:true}});
 f.line('LGHOME');f.line('LGHOME');f.requests[1].cb({payload:{returnValue:true}});
 assert.deepEqual(f.requests.map(x=>x.id),['com.openottplay.player.native','com.webos.app.home']);
}
// A queued short press must not displace a pending long-Home escape.
{
 const f=fixture();f.line('HOME');f.line('LGHOME');f.line('HOME');f.requests[0].cb({payload:{returnValue:true}});
 assert.equal(f.requests[1].id,'com.webos.app.home');
}
// Explicit pause cancels a confirmed escape, including after failure cleanup began.
for(const stage of ['before-failure','after-failure','cleanup-pending']){
 const f=fixture();f.line('HOME');f.line('LGHOME');
 if(stage!=='before-failure')f.requests[0].cb({payload:{returnValue:false}});
 if(stage==='cleanup-pending')f.worker.emit('exit',0);
 f.handlers.SIGTERM();
 if(stage==='before-failure')f.requests[0].cb({payload:{returnValue:true}});
 assert.equal(f.requests.length,1);assert.equal(f.worker.ended,'Q');
 if(stage!=='cleanup-pending')f.worker.emit('exit',0);assert.deepEqual(Array.from(f.children[1].args),['cleanup']);f.children[1].emit('exit',0);assert.equal(f.requests.length,1);
}
// A confirmed long-Home survives failed/timed-out OTT only after worker release
// AND key cleanup. A late OTT reply never retries OTT or duplicates LG Home.
for(const timeout of [false,true]){
 const f=fixture();f.line('HOME');f.advance(2000);f.line('LGHOME');
 if(timeout)f.lastTimer(3000).fn();else f.requests[0].cb({payload:{returnValue:false}});
 assert.equal(f.worker.ended,'Q');assert.equal(f.requests.length,1);
 f.worker.stdin.emit('error',new Error('closed after Q'));
 f.requests[0].cb({payload:{returnValue:true}});assert.equal(f.requests.length,1);
 f.worker.emit('exit',0);assert.deepEqual(Array.from(f.children[1].args),['cleanup']);assert.equal(f.requests.length,1);
 f.children[1].emit('exit',0);
 assert.deepEqual(f.requests.map(x=>x.id),['com.openottplay.player.native','com.webos.app.home']);
 f.requests[1].cb({payload:{returnValue:true}});assert.deepEqual(f.exits,[0]);
}
// An ordinary failed short-Home request does not manufacture an escape.
{
 const f=fixture();f.line('HOME');f.requests[0].cb({payload:{returnValue:false}});
 f.worker.emit('exit',0);f.children[1].emit('exit',0);assert.equal(f.requests.length,1);
}
// Eight-second native release clears pending intent, cleans up, then orders
// LG Home after the outstanding OTT request has settled.
{
 const f=fixture();f.line('HOME');f.line('LGHOME');f.worker.emit('exit',22);f.children[1].emit('exit',0);
 assert.equal(f.requests.length,1);f.requests[0].cb({payload:{returnValue:true}});f.lastTimer(50).fn();
 assert.deepEqual(f.requests.map(x=>x.id),['com.openottplay.player.native','com.webos.app.home']);
 f.requests[1].cb({payload:{returnValue:true}});assert.deepEqual(f.exits,[0]);
}
console.log('PASS: actual controller delayed OTT→LG escape, priority, pause cancellation, rejection/timeout escape preservation and 8-second cleanup/launch ordering. VM doubles only.');
