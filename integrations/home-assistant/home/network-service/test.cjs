'use strict';
const assert=require('node:assert/strict'),EventEmitter=require('node:events'),p=require('./probe');
for(const address of ['127.0.0.1','169.254.169.254','192.168.1.999','010.0.0.1','example.com','224.0.0.1'])assert.equal(p.allowed(address,'router'),false);
assert.equal(p.gateway({wired:{state:'connected',gateway:'192.168.1.1'}}),'192.168.1.1');
assert.equal(p.gateway({wired:{state:'connected',gateway:'1.2.3.4'}}),null);
assert.equal(p.valid({target:'router',port:443}),true);assert.equal(p.valid({target:'internet',port:53}),true);
assert.equal(p.valid({target:'router',port:22}),false);assert.equal(p.valid({target:'router',port:443,address:'192.168.1.2'}),false);
for(const kind of ['router','internet']){
  let socket,calls=0,destroyed=0;const fake={connect(o){assert.equal(o.host,kind==='router'?'192.168.1.1':'1.1.1.1');socket=new EventEmitter();socket.destroy=()=>{destroyed++;};return socket;}};
  p.tcp(fake,kind==='router'?'192.168.1.1':'1.1.1.1',443,kind,r=>{calls++;assert.equal(r.status,'ok');assert.equal(r.method,'tcp');assert.ok(r.ms>=0);});
  socket.emit('connect');socket.emit('error',Error('later'));socket.emit('close');assert.equal(calls,1);assert.equal(destroyed,1);
}
let connects=0;p.tcp({connect(){connects++;}},'127.0.0.1',443,'internet',r=>assert.equal(r.status,'unavailable'));assert.equal(connects,0);
console.log('PASS: gateway/address/argument restrictions and injected socket lifecycle; no network connections.');
