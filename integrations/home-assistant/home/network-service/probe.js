'use strict';
// Adapted from MBS Integrations network-service/probe.js @204d4ba, GPL-3.0-only.
function privateIPv4(v){var a=String(v).split('.');if(a.length!==4||a.some(function(x){return !/^(0|[1-9][0-9]{0,2})$/.test(x)||Number(x)>255;}))return false;a=a.map(Number);return (a[0]===10||a[0]===172&&a[1]>=16&&a[1]<=31||a[0]===192&&a[1]===168)&&a[3]>0&&a[3]<255;}
function allowed(address,kind){return kind==='router'?privateIPv4(address):kind==='internet'&&['1.1.1.1','8.8.8.8'].indexOf(address)!==-1;}
function valid(p){return !!p&&Object.keys(p).every(function(k){return k==='target'||k==='port';})&&['router','internet'].indexOf(p.target)!==-1&&[443,80,53].indexOf(p.port)!==-1;}
function gateway(p){if(p&&p.returnValue!==false){if(p.wired&&p.wired.state==='connected'&&privateIPv4(p.wired.gateway))return p.wired.gateway;if(p.wifi&&p.wifi.state==='connected'&&privateIPv4(p.wifi.gateway))return p.wifi.gateway;}return null;}
function tcp(net,address,port,kind,callback){
  if(!allowed(address,kind)||[443,80,53].indexOf(port)===-1){callback({status:'unavailable',code:'ENDPOINT_UNAVAILABLE',ms:null,method:'tcp'});return;}
  var socket,done=false,started=process.hrtime(),timer;
  function finish(status,code){if(done)return;done=true;clearTimeout(timer);var elapsed=process.hrtime(started);if(socket)socket.destroy();callback({status:status,code:code,ms:status==='ok'?elapsed[0]*1000+elapsed[1]/1000000:null,method:'tcp'});}
  timer=setTimeout(function(){finish('unavailable','TIMEOUT');},2500);
  try{socket=net.connect({host:address,port:port});socket.once('connect',function(){finish('ok');});socket.once('error',function(){finish('unavailable','CONNECT_FAILED');});socket.once('close',function(){finish('unavailable','CONNECT_FAILED');});}catch(e){finish('unavailable','CONNECT_FAILED');}
}
module.exports={privateIPv4:privateIPv4,allowed:allowed,valid:valid,gateway:gateway,tcp:tcp};
