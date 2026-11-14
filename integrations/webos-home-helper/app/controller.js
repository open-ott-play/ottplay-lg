'use strict';
// Adapted from MBS Integrations helper @204d4ba; GPL-3.0-only + UPSTREAM-LICENSE.
var fs=require('fs'),cp=require('child_process'),Service=require('/usr/lib/node_modules/webos-service');
var processState=require('./process-state'),policy=require('./policy'),root=__dirname,data=root+'/state';
var service=new Service('com.openottplay.homehelper'),options=JSON.parse(fs.readFileSync(root+'/options.json','utf8'));
var params=JSON.parse(process.argv[2]||'{}'),boot=fs.readFileSync('/proc/sys/kernel/random/boot_id','utf8').trim();
var shared='/media/developer/apps/usr/palm/services/com.openottplay.homehelper.control/state';
var child=null,ready=false,stopping=false,timer=null,workerTick=null,buffer='',lastAck=0,launching=false,pendingIntent=null,activeIntent=null;
var parentTick=processState.read(process.pid).tick,monotonicStart=process.hrtime();
function now(){var t=process.hrtime();return t[0]*1000+t[1]/1e6;}
function control(){try{return JSON.parse(fs.readFileSync(shared+'/control.json','utf8'));}catch(e){return null;}}
function allowed(){return policy.allowed(control(),params,boot,Date.now()) && !fs.existsSync(data+'/blocked');}
function report(released,code){var r={session:params.session,boot:boot,pid:process.pid,parentTick:parentTick,worker:child&&child.pid,workerTick:workerTick,active:ready&&!stopping,released:released,code:code,at:Date.now()};try{fs.writeFileSync(shared+'/native-status.json.tmp',JSON.stringify(r),{mode:432});fs.renameSync(shared+'/native-status.json.tmp',shared+'/native-status.json');}catch(e){return false;}return true;}
function launch(id,done){var finished=false,t=setTimeout(function(){if(!finished){finished=true;done(false);}},3000);service.call('luna://com.webos.applicationManager/launch',{id:id},function(m){if(finished)return;finished=true;clearTimeout(t);done(!!(m.payload&&m.payload.returnValue));});}
function block(){try{fs.writeFileSync(data+'/blocked','paused');}catch(e){}}
function requestIntent(id){
  if(stopping||!ready)return;
  if(launching){if(id===activeIntent){if(id==='com.webos.app.home')pendingIntent=null;return;}if(id==='com.webos.app.home'||pendingIntent===null)pendingIntent=id;return;}
  launching=true;activeIntent=id;
  launch(id,function(ok){
    launching=false;activeIntent=null;
    if(stopping)return;
    if(!ok){stop();return;}
    var next=pendingIntent;pendingIntent=null;if(next)requestIntent(next);
  });
}
function finish(code,signal){if(stopping==='finished')return;stopping='finished';pendingIntent=null;ready=false;clearInterval(timer);block();
  // Worker has exited: clear any relayed held keys before announcing release.
  var cleanup=cp.spawn(root+'/relay',['cleanup'],{stdio:'ignore'}),done=false,guard=setTimeout(function(){try{cleanup.kill('SIGKILL');}catch(e){}},1500);
  function completed(){if(done)return;done=true;clearTimeout(guard);report(true,code);
    if(code!==22){process.exit(code||0);return;}
    // The 8-second escape has already released input. Order the LG Home
    // request after the prior bounded foreground request settles, not beside it.
    var until=now()+3200;
    function escape(){if(launching&&now()<until){setTimeout(escape,50);return;}launch('com.webos.app.home',function(){process.exit(0);});}
    escape();
  }
  cleanup.on('exit',completed);cleanup.on('error',completed);
}
function stop(){if(stopping)return;stopping=true;pendingIntent=null;ready=false;block();if(child){try{child.stdin.end('Q');}catch(e){}setTimeout(function(){try{child.kill('SIGKILL');}catch(e){}},1500);}else finish(0);}
process.on('SIGTERM',stop);process.on('SIGINT',stop);process.on('uncaughtException',stop);
if(!policy.identity(options,process.getuid(),process.getgid(),process.getgroups()) || !params.session || !parentTick){process.exit(3);}
try{fs.unlinkSync(data+'/blocked');}catch(e){if(e.code!=='ENOENT')process.exit(4);}
if(!allowed() || params.mode!==control().mode || params.mode==='trial' && (params.trialSeconds%1!==0 || params.trialSeconds<1 || params.trialSeconds>120)){block();process.exit(5);}
if(!report(false,null)){block();process.exit(8);}
child=cp.spawn(root+'/relay',[],{stdio:['pipe','pipe','ignore']});workerTick=processState.read(child.pid).tick;lastAck=now();
if(!workerTick){try{child.kill('SIGKILL');}catch(e){}process.exit(6);}
child.stdin.on('error',stop);child.on('error',function(){stop();finish(7);});child.on('exit',finish);
child.stdout.on('data',function(b){buffer+=b; if(buffer.length>4096){stop();return;}var i;while((i=buffer.indexOf('\n'))>=0){var line=buffer.slice(0,i);buffer=buffer.slice(i+1);if(line==='P')lastAck=now();if(line==='READY'){ready=true;if(!report(false,null))stop();}if(line==='HOME'||line==='LGHOME')requestIntent(line==='HOME'?'com.openottplay.player.native':'com.webos.app.home');}});
timer=setInterval(function(){
  if(stopping)return;
  if(!allowed() || !processState.matches(processState.read(child.pid),workerTick) || now()-lastAck>2500){stop();return;}
  // The controller/worker heartbeat is monotonic. Trial cannot outlive 120 s
  // even if the wall clock or service deadline changes.
  var elapsed=process.hrtime(monotonicStart);
  if(params.mode==='trial' && elapsed[0]*1000+elapsed[1]/1e6>=params.trialSeconds*1000){stop();return;}
  child.stdin.write('P');if(!report(false,null))stop();
},400);
