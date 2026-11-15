'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {Readable} = require('node:stream');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {configuration,HomeAssistant,createHandler} = require('./server.cjs');
const KEY = 'client_key_0123456789_0123456789_0123456789';
const raw = () => ({version:1,homeAssistant:{url:'https://ha.example.invalid',token:'PRIVATE_HA_TOKEN'},
  accessKey:KEY,allowedOrigins:['https://player.example.invalid'],
  entities:[{id:'temp',entity:'sensor.room',label:'Room',kind:'temperature'}],
  actions:[{id:'light',entity:'light.room',label:'Light',service:'light.toggle'}],
  apps:[{id:'hdmi',entity:'media_player.tv',label:'HDMI',kind:'hdmi',source:'HDMI 1'}]});
class Socket extends EventTarget {
  constructor(url) { super(); this.url = url; this.sent = []; Socket.latest = this; }
  send(data) { this.sent.push(JSON.parse(data)); }
  message(data) { this.dispatchEvent(new MessageEvent('message',{data:JSON.stringify(data)})); }
  close() { this.dispatchEvent(new Event('close')); }
}
function fixture() {
  let now = 1000; const timers = [];
  const home = new HomeAssistant(configuration(raw()),{WebSocket:Socket,now:() => now,
    schedule:(fn,delay) => { const t = {fn,delay}; timers.push(t); return t; },cancel:t => { t.cancelled = true; }});
  home.start(); const socket = Socket.latest;
  socket.message({type:'auth_required'}); socket.message({type:'auth_ok'});
  function answer(request,result,success = true) { socket.message({type:'result',id:request.id,success,result}); }
  return {home,socket,timers,setNow:n => { now = n; },answer};
}
const tick = () => new Promise(resolve => setImmediate(resolve));
test('configuration rejects credential URLs, insecure default, wildcards, arbitrary services and duplicate IDs',() => {
  for (const mutate of [r => {r.homeAssistant.url='https://user:pass@ha.invalid';},
    r => {r.homeAssistant.url='http://ha.invalid';},r => {r.homeAssistant.url='https://ha.invalid/proxy';},
    r => {r.allowedOrigins=['*'];},r => {r.accessKey='short';},
    r => {r.actions[0].service='homeassistant.restart';},r => {r.actions[0].entity='switch.room';},
    r => {r.apps[0].id='light';}]) {
    const r = raw(); mutate(r); assert.throws(() => configuration(r));
  }
  assert.equal(configuration(raw()).haOrigin,'https://ha.example.invalid');
});
test('credentials stay on HA socket; selected snapshots omit full attributes and private tokens',async () => {
  const f = fixture(); assert.deepEqual(f.socket.sent[0],{type:'auth',access_token:'PRIVATE_HA_TOKEN'});
  const get = f.socket.sent.find(x => x.type === 'get_states');
  f.answer(get,[{entity_id:'sensor.room',state:'21',attributes:{unit_of_measurement:'°C',secret:'DONOTEXPORT'}},
    {entity_id:'sensor.other',state:'PRIVATE_OTHER',attributes:{}}]); await tick();
  const snapshot = f.home.snapshot();
  assert.equal(snapshot.entities[0].state,'21'); assert.equal(snapshot.entities[0].unit,'°C');
  const encoded = JSON.stringify(snapshot); for (const secret of ['PRIVATE_HA_TOKEN','DONOTEXPORT','PRIVATE_OTHER','sensor.room']) assert.ok(!encoded.includes(secret));
  f.home.stop();
});
test('event received while snapshot is pending wins, including deletion',async () => {
  const f = fixture(); const get = f.socket.sent.find(x => x.type === 'get_states');
  f.socket.message({type:'event',event:{data:{entity_id:'sensor.room',new_state:{entity_id:'sensor.room',state:'new'}}}});
  f.answer(get,[{entity_id:'sensor.room',state:'old'}]); await tick();
  assert.equal(f.home.snapshot().entities[0].state,'new');
  const refresh = f.home.refresh(); const latest = f.socket.sent.at(-1);
  f.socket.message({type:'event',event:{data:{entity_id:'sensor.room',new_state:null}}});
  f.answer(latest,[{entity_id:'sensor.room',state:'resurrected'}]); await refresh;
  assert.equal(f.home.snapshot().entities[0].state,'unavailable'); f.home.stop();
});
test('actions accept only configured service/entity and source-list-approved app',async () => {
  const f = fixture(); await assert.rejects(f.home.action('arbitrary'),/unknown_action/);
  await assert.rejects(f.home.action('hdmi'),/source_unavailable/);
  const action = f.home.action('light'); const request = f.socket.sent.at(-1);
  assert.deepEqual(request,{id:request.id,type:'call_service',domain:'light',service:'toggle',target:{entity_id:'light.room'}});
  f.answer(request,{}); assert.deepEqual(await action,{version:1,status:'requested'});
  f.socket.message({type:'event',event:{data:{entity_id:'media_player.tv',new_state:{entity_id:'media_player.tv',attributes:{source_list:['HDMI 1']}}}}});
  const launch = f.home.action('hdmi'); const source = f.socket.sent.at(-1);
  assert.deepEqual(source.service_data,{source:'HDMI 1'}); f.answer(source,{}); await launch; f.home.stop();
});
test('stale, disconnected and stopped data never authorize actions or accept old messages',async () => {
  const f = fixture(); f.setNow(46001); assert.equal(f.home.snapshot().available,false);
  await assert.rejects(f.home.action('light'),/home_unavailable/);
  f.home.stop(); f.socket.message({type:'event',event:{data:{entity_id:'sensor.room',new_state:{state:'late'}}}});
  assert.equal(f.home.snapshot().available,false); assert.equal(f.home.states.size,0);
});
test('malformed messages are ignored and a timed-out owner reconnects without replaying effects',async () => {
  const f = fixture();
  for (const value of [null,[],1,'bad']) assert.doesNotThrow(() => f.socket.message(value));
  const action = f.home.action('light').catch(error => error.message);
  const sent = f.socket.sent.filter(x => x.type === 'call_service'); assert.equal(sent.length,1);
  const pending = f.home.pending.get(sent[0].id); pending.timer.fn();
  assert.equal(await action,'home_unavailable'); assert.equal(f.home.ready,false); assert.equal(f.home.socket,null);
  const reconnect = f.timers.find(t => !t.cancelled && t.delay === 1000); assert.ok(reconnect);
  reconnect.fn();
  assert.notEqual(f.home.socket,f.socket); assert.equal(f.home.socket.sent.length,0,'no replay on a new connection');
  f.socket.message({type:'auth_ok'}); assert.equal(f.home.ready,false,'old socket cannot authorize replacement');
  f.home.stop();
});
test('opaque packaged origins require explicit opt-in and client and HA keys must differ',() => {
  const config = raw(); config.allowedOrigins=['null']; assert.throws(() => configuration(config));
  config.allowPackagedOrigin=true; assert.deepEqual(configuration(config).origins,['null']);
  config.homeAssistant.token=KEY; assert.throws(() => configuration(config));
});
test('throwing auth send is contained and invalid startup port exits before any HA connection',() => {
  const f = fixture(); f.socket.send=() => { throw new Error('broken'); };
  assert.doesNotThrow(() => f.socket.message({type:'auth_required'}));
  assert.equal(f.home.ready,false); assert.equal(f.home.socket,null); f.home.stop();
  const dir = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()),'ott-home-config-'));
  try {
    const file=path.join(dir,'config.json'); fs.writeFileSync(file,JSON.stringify(raw()),{mode:0o600});
    const result=spawnSync(process.execPath,[path.join(__dirname,'server.cjs')],{
      env:{...process.env,OTT_HOME_CONFIG:file,OTT_HOME_PORT:'0'},encoding:'utf8',timeout:1500});
    assert.equal(result.status,1); assert.equal(result.error,undefined);
    assert.ok(!result.stderr.includes('PRIVATE_HA_TOKEN')); assert.equal(result.stdout,'');
  } finally { fs.rmSync(dir,{recursive:true,force:true}); }
});
test('bounded pending queue and reconnect do not create duplicate reconnect timers',async () => {
  const f = fixture(); const pending = [];
  for (let i = 0; i < 8; i++) pending.push(f.home.request('get_states').catch(e => e.message));
  await assert.rejects(f.home.request('get_states'),/home_unavailable/);
  f.socket.close(); f.socket.close(); await Promise.all(pending);
  const reconnect = f.timers.filter(t => !t.cancelled && t.delay === 1000);
  assert.equal(reconnect.length,1); f.home.stop(); assert.equal(reconnect[0].cancelled,true);
});
async function invoke(handler,{method='GET',url='/api/home',origin='https://player.example.invalid',key=KEY,body='',type='application/json'} = {}) {
  const req = Readable.from(body ? [Buffer.from(body)] : []);
  Object.assign(req,{method,url,headers:{origin,authorization:'Bearer '+key,'content-type':type}});
  const headers = {}; let finish; const result = new Promise(resolve => { finish = resolve; });
  const res = {statusCode:200,setHeader:(k,v) => {headers[k]=v;},end:value => finish({status:res.statusCode,headers,body:value ? headers['Content-Type']?.startsWith('application/json') ? JSON.parse(value) : String(value) : null})};
  await handler(req,res); return result;
}
test('HTTP auth/CORS reject before effects; preflight has no HA call',async () => {
  let calls = 0; const handler = createHandler(configuration(raw()),{refresh:async () => {calls++;},snapshot:() => ({version:1}),action:async () => {calls++;}});
  assert.equal((await invoke(handler,{key:'wrong'})).status,401);
  assert.equal((await invoke(handler,{origin:'https://evil.invalid'})).status,403);
  assert.equal((await invoke(handler,{method:'OPTIONS'})).status,204); assert.equal(calls,0);
  const ok = await invoke(handler); assert.equal(ok.status,200); assert.equal(calls,1);
  assert.equal(ok.headers['Access-Control-Allow-Origin'],'https://player.example.invalid');
});
test('HTTP action rejects malformed/excess/oversized fields with zero dispatch',async () => {
  let calls = 0; const handler = createHandler(configuration(raw()),{action:async () => {calls++;return {};}});
  for (const body of ['null','{}','{"id":"light","entity":"light.other"}','{"id":"../light"}','x'.repeat(1025)]) {
    const response = await invoke(handler,{method:'POST',url:'/api/action',body}); assert.ok([400,413].includes(response.status));
  }
  assert.equal((await invoke(handler,{method:'POST',url:'/api/action',body:'{"id":"light"}',type:'text/plain'})).status,415);
  assert.equal(calls,0);
  assert.equal((await invoke(handler,{method:'POST',url:'/api/action',body:'{"id":"light"}'})).status,200); assert.equal(calls,1);
});
test('static dashboard whitelist serves required local Palm bridge but never private files',async () => {
  const handler=createHandler(configuration(raw()),{}, {dashboardRoot:path.join(__dirname,'../dashboard')});
  const script=await invoke(handler,{url:'/webos-host.js',key:'not-needed-for-public-assets'});
  assert.equal(script.status,200); assert.ok(script.body.includes('PalmServiceBridge'));
  assert.ok(script.headers['Content-Security-Policy'].includes("frame-ancestors 'self' https://player.example.invalid"));
  assert.ok(!script.body.includes('PRIVATE_HA_TOKEN'));
  for(const url of ['/config.json','/../bridge/config.example.json','/server.cjs'])
    assert.equal((await invoke(handler,{url,key:'wrong'})).status,401);
});

test('concurrent refresh is single-flight and older duplicate results cannot roll back a newer snapshot',async () => {
  const f=fixture();
  const initial=f.socket.sent.find(x=>x.type==='get_states');
  const first=f.home.refresh(),second=f.home.refresh();
  assert.equal(first,second);
  assert.equal(f.socket.sent.filter(x=>x.type==='get_states').length,1);
  f.answer(initial,[{entity_id:'sensor.room',state:'initial'}]);await Promise.all([first,second]);
  const newer=f.home.refresh();const request=f.socket.sent.at(-1);
  assert.notEqual(request.id,initial.id);
  assert.equal(f.home.refresh(),newer);
  f.answer(request,[{entity_id:'sensor.room',state:'new'}]);await newer;
  f.answer(initial,[{entity_id:'sensor.room',state:'old'}]);
  assert.equal(f.home.snapshot().entities[0].state,'new');
  const deleted=f.home.refresh();const deletionRequest=f.socket.sent.at(-1);
  f.socket.message({type:'event',event:{data:{entity_id:'sensor.room',new_state:null}}});
  f.answer(deletionRequest,[{entity_id:'sensor.room',state:'resurrected'}]);await deleted;
  assert.equal(f.home.snapshot().entities[0].state,'unavailable');f.home.stop();
});
test('old refresh cleanup cannot clear the replacement connection single-flight owner',async () => {
  const f=fixture();const old=f.home.refresh().catch(error=>error.message);
  f.socket.close();
  const reconnect=f.timers.find(t=>!t.cancelled&&t.delay===1000);reconnect.fn();
  const socket=Socket.latest;socket.message({type:'auth_ok'});
  const owner=f.home.refreshPending;
  assert.ok(owner);assert.equal(await old,'home_unavailable');
  assert.equal(f.home.refreshPending,owner);
  assert.equal(f.home.refresh(),owner.promise);
  assert.equal(socket.sent.filter(x=>x.type==='get_states').length,1);
  const request=socket.sent.find(x=>x.type==='get_states');
  socket.message({type:'result',id:request.id,success:true,result:[{entity_id:'sensor.room',state:'replacement'}]});
  await owner.promise;
  assert.equal(f.home.snapshot().entities[0].state,'replacement');f.home.stop();
});
test('public static assets reject leaf and ancestor symlinks and nonregular files',async () => {
  const root=fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()),'ott-home-static-custody-'));
  try {
    const assets=path.join(root,'assets');fs.mkdirSync(assets);
    const privateFile=path.join(root,'private.json');fs.writeFileSync(privateFile,'PRIVATE_SENTINEL',{mode:0o600});
    fs.symlinkSync(privateFile,path.join(assets,'app.js'));
    fs.writeFileSync(path.join(assets,'index.html'),'public');
    const handler=createHandler(configuration(raw()),{}, {dashboardRoot:assets});
    const leaf=await invoke(handler,{url:'/app.js',key:'public'});
    assert.equal(leaf.status,404);assert.ok(!JSON.stringify(leaf).includes('PRIVATE_SENTINEL'));
    const alias=path.join(root,'alias');fs.symlinkSync(assets,alias,'dir');
    const parent=createHandler(configuration(raw()),{}, {dashboardRoot:alias});
    assert.equal((await invoke(parent,{url:'/',key:'public'})).status,404);
    fs.unlinkSync(path.join(assets,'app.js'));fs.linkSync(privateFile,path.join(assets,'app.js'));
    assert.equal((await invoke(handler,{url:'/app.js',key:'public'})).status,404);
    assert.equal(fs.readFileSync(privateFile,'utf8'),'PRIVATE_SENTINEL');
    fs.unlinkSync(path.join(assets,'app.js'));fs.mkdirSync(path.join(assets,'app.js'));
    assert.equal((await invoke(handler,{url:'/app.js',key:'public'})).status,404);
    assert.equal((await invoke(handler,{url:'/',key:'public'})).body,'public');
  } finally {fs.rmSync(root,{recursive:true,force:true});}
});

// Deliberately limited to this context's exact paths, ** and prefix/** rules.
// Parent matching follows Moby MatchesOrParentMatches, not gitignore semantics:
// https://github.com/moby/patternmatcher/blob/main/patternmatcher.go
// https://docs.docker.com/build/concepts/context/
function homeContextExcluded(patterns,file) {
  assert.match(file,/^[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)*$/);
  assert.ok(file.split('/').every(part=>part!=='.'&&part!=='..'));
  const parts=file.split('/'),candidates=[file];
  for(let end=1;end<parts.length;end++)candidates.push(parts.slice(0,end).join('/'));
  let excluded=false;
  for(let rule of patterns.split(/\r?\n/).map(line=>line.trim()).filter(line=>line&&!line.startsWith('#'))){
    const exception=rule.startsWith('!');if(exception)rule=rule.slice(1);
    rule=rule.replace(/^\/+|\/+$/g,'');
    assert.match(rule,/^(?:\*\*|[A-Za-z0-9_.-]+(?:\/[A-Za-z0-9_.-]+)*(?:\/\*\*)?)$/,'unsupported pattern must not silently use an approximate matcher');
    if(exception!==excluded)continue;
    const matches=candidates.some(candidate=>rule==='**'||(rule.endsWith('/**')?candidate.startsWith(rule.slice(0,-2)):candidate===rule));
    if(matches)excluded=!exception;
  }
  return excluded;
}
test('Docker context exceptions re-exclude descendants before admitting fixed public files',()=>{
  const current=fs.readFileSync(path.join(__dirname,'../.dockerignore'),'utf8');
  const leaked=['bridge/config.private.json','bridge/nested/secrets.json','bridge/.env',
    'dashboard/config.json','dashboard/nested/credentials.json','dashboard/.env'];
  const previous=current.split(/\r?\n/).filter(line=>!['bridge/**','dashboard/**'].includes(line.trim())).join('\n');
  assert.notEqual(previous,current);
  // Negative control: a directory exception alone includes descendants through
  // Moby's parent matching. The old policy must demonstrably fail this check.
  for(const file of leaked){assert.equal(homeContextExcluded(previous,file),false,'old policy leaked '+file);assert.equal(homeContextExcluded(current,file),true,'current policy must exclude '+file);}
  for(const file of ['LICENSE','bridge/Dockerfile','bridge/server.cjs','dashboard/index.html',
    'dashboard/style.css','dashboard/core.js','dashboard/webos-host.js','dashboard/app.js'])
    assert.equal(homeContextExcluded(current,file),false,'required input excluded: '+file);
  for(const file of ['bridge/server.test.cjs','bridge/REVIEW-FIXES.json','dashboard/core.test.cjs',
    'network-service/config.json','portable-home/package-webos.test.cjs','config.private.json'])
    assert.equal(homeContextExcluded(current,file),true,'nonruntime input admitted: '+file);
  assert.throws(()=>homeContextExcluded('*','bridge/server.cjs'));
});
