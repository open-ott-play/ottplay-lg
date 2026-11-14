'use strict';
// Run the actual service registration against in-memory FS/Luna/process doubles.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const base=path.resolve(__dirname,'..'),policy=require('../service/policy'),processState=require('../service/process-state');
const files=new Map(),methods={},timers=[],actors=new Map();let launches=0,serial=0;
const data='/fixture/state',clock=100000;let boot='fixture-boot';
const fakeFs={readFileSync(p){if(p==='/proc/sys/kernel/random/boot_id')return boot;if(!files.has(p))throw Object.assign(Error('missing'),{code:'ENOENT'});return files.get(p);},writeFileSync(p,v){files.set(p,v);},renameSync(a,b){files.set(b,files.get(a));files.delete(a);}};
function Service(){this.activityManager={};this.register=(name,fn)=>{methods[name]=fn;};this.call=(uri,p,done)=>{assert.equal(uri,'luna://com.webos.applicationManager/launch');assert.equal(p.id,'com.openottplay.homehelper');launches++;done({payload:{returnValue:true}});};}
const ps={...processState,read(pid){return actors.get(pid)||{gone:true};}};
const env={__dirname:'/fixture',require(n){return {'fs':fakeFs,'crypto':{randomBytes(){return Buffer.alloc(16,++serial);}},'webos-service':Service,'./process-state':ps,'./policy':policy}[n]||(()=>{throw Error('Unexpected import '+n);})();},Date:{now:()=>clock},setTimeout(fn){const t={fn};timers.push(t);return t;},clearTimeout(t){if(t)t.cleared=true;}};
vm.runInNewContext(fs.readFileSync(path.join(base,'service/service.js'),'utf8'),env);
function call(method,payload={}){let answer;methods[method]({payload,respond(v){answer=v;}});return ()=>answer;}
assert.equal(call('status')().state.enabled,false);assert.equal(call('ensure')().returnValue,false);assert.equal(launches,0);
const trial=call('trial',{trialSeconds:30})();assert.equal(trial.returnValue,true);assert.equal(trial.state.active,false);assert.equal(launches,1);
assert.equal(call('trial',{trialSeconds:30})().returnValue,false,'Pending launch cannot mint another session');
const s=JSON.parse(files.get(data+'/control.json'));
actors.set(11,{tick:'p',state:'S'});actors.set(12,{tick:'w',state:'S'});
files.set(data+'/native-status.json',JSON.stringify({session:s.session,boot,pid:11,parentTick:'p',worker:12,workerTick:'w',active:true,released:false,at:clock}));
assert.equal(call('status')().state.active,true);assert.equal(JSON.parse(files.get(data+'/control.json')).lastTrial.active,true);
assert.equal(call('trial',{trialSeconds:30})().returnValue,false,'No competing controller');
const paused=call('pause');assert.equal(paused(),undefined);actors.clear();timers.filter(t=>!t.cleared).at(-1).fn();assert.equal(paused().state.released,true);
assert.equal(call('enable',{acceptTestedControls:true,testedTrialSession:'wrong'})().returnValue,false);
assert.equal(call('enable',{acceptTestedControls:true,testedTrialSession:s.session})().returnValue,true);assert.equal(launches,2);
assert.equal(call('status')().state.enabled,true);assert.equal(launches,2,'status never launches');
const enabled=JSON.parse(files.get(data+'/control.json'));
actors.set(21,{tick:'ep',state:'S'});actors.set(22,{tick:'ew',state:'S'});
files.set(data+'/native-status.json',JSON.stringify({session:enabled.session,boot,pid:21,parentTick:'ep',worker:22,workerTick:'ew',active:true,released:false,at:clock}));
assert.equal(call('status')().state.active,true);actors.clear();
files.set(data+'/native-status.json',JSON.stringify({session:enabled.session,boot,pid:21,parentTick:'ep',worker:22,workerTick:'ew',active:false,released:true,code:22,at:clock}));
const afterEscape=call('ensure')();assert.equal(afterEscape.returnValue,false);assert.equal(afterEscape.state.paused,true);assert.equal(launches,2,'ensure must not undo native emergency pause');
assert.equal(call('enable',{acceptTestedControls:true,testedTrialSession:s.session})().returnValue,true);
const resumed=JSON.parse(files.get(data+'/control.json'));actors.set(31,{tick:'rp',state:'S'});actors.set(32,{tick:'rw',state:'S'});
files.set(data+'/native-status.json',JSON.stringify({session:resumed.session,boot,pid:31,parentTick:'rp',worker:32,workerTick:'rw',active:true,released:false,at:clock}));
assert.equal(call('status')().state.active,true);
// Current-boot control must not treat a stale report from another boot as
// absence. Admission is centralized before trial/enable mutate the session.
files.set(data+'/native-status.json',JSON.stringify({session:resumed.session,boot:'stale-foreign-boot',pid:31,parentTick:'rp',worker:32,workerTick:'rw',active:false,released:true,at:clock}));
assert.equal(call('status')().state.ownershipUnknown,true);
const beforeStale=launches,controlBeforeStale=files.get(data+'/control.json');
assert.equal(call('ensure')().returnValue,false);
assert.equal(call('trial',{trialSeconds:30})().returnValue,false);
assert.equal(call('enable',{acceptTestedControls:true,testedTrialSession:s.session})().returnValue,false);
assert.equal(launches,beforeStale);assert.equal(files.get(data+'/control.json'),controlBeforeStale,'Unknown ownership cannot mint a session');
files.delete(data+'/native-status.json');
assert.equal(call('status')().state.ownershipUnknown,true);assert.equal(call('status')().state.released,false);
const beforeMissing=launches;
assert.equal(call('ensure')().returnValue,false);assert.equal(call('trial',{trialSeconds:30})().returnValue,false);assert.equal(call('enable',{acceptTestedControls:true,testedTrialSession:s.session})().returnValue,false);assert.equal(launches,beforeMissing);
// New boot is the explicit kernel-lifetime boundary, not a guessed PID timeout.
boot='new-boot';vm.runInNewContext(fs.readFileSync(path.join(base,'service/service.js'),'utf8'),env);
assert.equal(call('status')().state.ownershipUnknown,false);assert.equal(launches,beforeMissing,'boot/status cannot auto-launch');
assert.equal(call('trial',{trialSeconds:30})().returnValue,true);assert.equal(launches,beforeMissing+1);
console.log('PASS: actual service default pause, explicit trial/acceptance, process ownership, pause release, and status without launch. In-memory doubles only.');
