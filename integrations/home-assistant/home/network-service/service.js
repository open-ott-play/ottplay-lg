'use strict';
// Optional local service. No shell, ICMP, scanning, privileged helper or polling.
var net=require('net'),Service=require('webos-service'),probe=require('./probe'),config=require('./network.json');
var service=new Service('com.openottplay.home.network'),busy=false,lastAt=0;
service.register('probe',function(message){
  var p=message.payload;
  function reject(code){message.respond({returnValue:false,status:'unavailable',code:code,ms:null});}
  if(config.enabled!==true){reject('DISABLED');return;}
  if(!probe.valid(p)){reject('INVALID_ARGUMENT');return;}
  if(busy){reject('BUSY');return;}
  if(Date.now()-lastAt<Math.max(15000,Number(config.minimumIntervalMs)||15000)){reject('RATE_LIMITED');return;}
  busy=true;lastAt=Date.now();
  function run(address){probe.tcp(net,address,p.port,p.target,function(result){busy=false;result.returnValue=true;result.target=p.target;result.port=p.port;message.respond(result);});}
  if(p.target==='internet'){run(config.internetTarget);return;}
  var finished=false,handle,timer=setTimeout(function(){received(null);},1500);
  function received(payload){if(finished)return;finished=true;clearTimeout(timer);if(handle)try{handle.cancel();}catch(e){}run(probe.gateway(payload));}
  try{handle=service.call('luna://com.palm.connectionmanager/getStatus',{},function(r){received(r.payload);});}catch(e){received(null);}
});
