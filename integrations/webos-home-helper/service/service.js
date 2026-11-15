'use strict';
// Explicit trial/enable only. Merely opening status never grabs the remote.
var fs=require('fs'),crypto=require('crypto'),Service=require('webos-service'),processState=require('./process-state'),policy=require('./policy');
var service=new Service('com.openottplay.homehelper.control'),data=__dirname+'/state',file=data+'/control.json';
service.activityManager.idleTimeout=130;
var boot=fs.readFileSync('/proc/sys/kernel/random/boot_id','utf8').trim(),s;
try{s=JSON.parse(fs.readFileSync(file,'utf8'));}catch(e){s={enabled:false,paused:true,accepted:false};}
function save(){fs.writeFileSync(file+'.tmp',JSON.stringify(s),{mode:432});fs.renameSync(file+'.tmp',file);}
function native(){try{return JSON.parse(fs.readFileSync(data+'/native-status.json','utf8'));}catch(e){return null;}}
function absent(r){
  if(!r)return !s.session||s.boot!==boot;
  if(r.boot!==boot)return true;
  if(s.session&&s.boot===boot&&r.session!==s.session)return false;
  if(!r.pid||!r.parentTick)return false;
  var parentGone=processState.gone(processState.read(r.pid),r.parentTick);
  // A pre-worker report cannot establish the identity of a later child. No
  // elapsed-time guess substitutes for its release report or a fresh boot.
  if(!r.workerTick)return !!(r.released&&parentGone);
  return parentGone&&processState.gone(processState.read(r.worker),r.workerTick);
}
function view(){var r=native(),same=r&&r.session===s.session&&r.boot===boot;var active=!!(same&&r.active&&processState.matches(processState.read(r.pid),r.parentTick)&&processState.matches(processState.read(r.worker),r.workerTick)&&Date.now()-r.at<2500);if(active&&s.starting){s.starting=false;save();}if(active&&s.mode==='trial'&&s.lastTrial&&!s.lastTrial.active){s.lastTrial.active=true;save();}if(same&&absent(r)&&(s.starting||!s.paused)){s.paused=true;s.starting=false;save();}return {enabled:!!s.enabled,paused:!!s.paused,active:active,starting:!!(s.starting&&!active&&!s.paused),session:s.session||null,lastTrial:s.lastTrial||null,released:!(s.starting&&s.boot===boot&&!same)&&absent(r),ownershipUnknown:!!(s.session&&s.boot===boot&&!same)};}
function respond(m,ok,error){m.respond({returnValue:ok,state:view(),errorText:error});}
service.register('status',function(m){respond(m,true);});
function start(m,mode){try{
  var observed=view();
  if(observed.ownershipUnknown){respond(m,false,'Previous process ownership is unknown');return;}
  var r=native();
  if(s.starting&&s.boot===boot){respond(m,false,'Previous launch ownership is unresolved; do not start a second controller');return;}
  if(!absent(r)){respond(m,false,'Pause and confirm prior process release before starting');return;}
  if(mode==='enabled'&&!policy.acceptance(s,m.payload)){respond(m,false,'A physically accepted, previously active trial session is required');return;}
  var seconds=mode==='trial'?policy.seconds(m.payload.trialSeconds):0;
  var previous=s.lastTrial||null,session=crypto.randomBytes(16).toString('hex'),at=Date.now();
  s={session:session,boot:boot,enabled:true,paused:false,accepted:mode==='enabled',mode:mode,started:at,deadline:mode==='trial'?at+seconds*1000:0,starting:true,lastTrial:mode==='trial'?{session:session,active:false}:previous};save();
  service.call('luna://com.webos.applicationManager/launch',{id:'com.openottplay.homehelper',params:{session:session,mode:mode,trialSeconds:seconds}},function(reply){if(s.session!==session)return;if(!reply.payload||!reply.payload.returnValue){s.paused=true;s.starting=false;save();}});
  // Request acceptance is not READY. status verifies actual PID start ticks.
  respond(m,true);
}catch(e){respond(m,false,'Helper start rejected');}}
service.register('trial',function(m){start(m,'trial');});
service.register('enable',function(m){start(m,'enabled');});
service.register('ensure',function(m){
  var observed=view();
  if(observed.ownershipUnknown){respond(m,false,'Previous process ownership is unknown');return;}
  // Optional external startup may call ensure only after explicit acceptance.
  if(!s.accepted||s.paused){respond(m,false,'Helper is not explicitly enabled');return;}
  if(!absent(native())){respond(m,true);return;}
  m.payload={acceptTestedControls:true,testedTrialSession:s.lastTrial&&s.lastTrial.session};start(m,'enabled');
});
service.register('pause',function(m){try{s.paused=true;save();var until=Date.now()+4500;function check(){if(view().released){respond(m,true);return;}if(Date.now()>=until){respond(m,false,'Release not confirmed; use the 8-second Home escape');return;}setTimeout(check,100);}check();}catch(e){respond(m,false,'Pause failed');}});
