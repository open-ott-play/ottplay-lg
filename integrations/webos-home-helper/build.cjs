'use strict';
// Local source staging and explicit packaging only; never installs or contacts a TV.
const fs=require('node:fs'),path=require('node:path'),cp=require('node:child_process'),crypto=require('node:crypto');
const ROOT=__dirname,APP='com.openottplay.homehelper',SERVICE=APP+'.control';
function validate(o,mode){
  if(!o||typeof o!=='object'||Object.keys(o).some(k=>!['developerUid','nativeUid','sharedGid','model','firmware','target','sourceName','destinationName','homeKey'].includes(k)))throw Error('Unknown config fields');
  if(!['probe','helper'].includes(mode))throw Error('Use probe or helper');
  for(const k of ['developerUid','sharedGid'].concat(mode==='helper'?['nativeUid']:[]))if(!Number.isInteger(o[k])||o[k]<1||o[k]>65534)throw Error('Set verified non-root '+k);
  if(o.target!=='arm-linux-musleabi')throw Error('Only the reviewed 32-bit ARM ABI is supported');
  for(const k of ['sourceName','destinationName'])if(typeof o[k]!=='string'||!/^[A-Za-z0-9 _\[\].-]{1,100}$/.test(o[k]))throw Error('Invalid input device name');
  if(o.sourceName===o.destinationName)throw Error('Input devices must differ');
  if(!Number.isInteger(o.homeKey)||o.homeKey<1||o.homeKey>1279)throw Error('Invalid Home key');
  for(const k of ['model','firmware'])if(typeof o[k]!=='string'||!o[k].trim()||o[k].length>100)throw Error('Record exact TV '+k);
  return o;
}
function readJson(file){const s=fs.lstatSync(file);if(!s.isFile()||s.isSymbolicLink()||s.size>16384)throw Error('Expected small regular JSON file');return JSON.parse(fs.readFileSync(file,'utf8'));}
function checkProbe(o,p){if(!p||p.uid!==o.nativeUid||p.gid!==o.sharedGid||p.compatible!==true||p.inputGrabbed!==false||p.eventsWritten!==0||p.eventBytes!==16||p.pointerBytes!==4||p.sourceName!==o.sourceName||p.destinationName!==o.destinationName)throw Error('Probe identity/capabilities must match this device configuration');}
function tool(name){const p=process.env[name];if(!p||!path.isAbsolute(p)||!fs.statSync(p).isFile())throw Error('Set absolute '+name);return p;}
function run(exe,args){const r=cp.spawnSync(exe,args,{stdio:'pipe',timeout:60000,maxBuffer:1024*1024});if(r.error||r.status!==0)throw Error('Local tool failed: '+path.basename(exe));return String(r.stdout).trim();}
function copy(src,dst){fs.cpSync(src,dst,{recursive:true,filter:p=>!fs.lstatSync(p).isSymbolicLink()});}
function stage(mode,o,out,receipt){
  validate(o,mode);if(mode==='helper')checkProbe(o,receipt);
  if(!path.isAbsolute(out))throw Error('Output must be absolute');
  fs.mkdirSync(out,{mode:0o700});
  const app=path.join(out,'app'),native=path.join(out,'native'),service=path.join(out,'service');
  copy(path.join(ROOT,'native'),native);copy(path.join(ROOT,'app'),app);
  if(mode==='helper')copy(path.join(ROOT,'service'),service);
  for(const dir of mode==='helper'?[app,service]:[app]){fs.mkdirSync(path.join(dir,'state'));fs.writeFileSync(path.join(dir,'state/.keep'),'');}
  fs.writeFileSync(path.join(native,'device-config.h'),'#define SOURCE_NAME '+JSON.stringify(o.sourceName)+'\n#define DESTINATION_NAME '+JSON.stringify(o.destinationName)+'\n#define HOME_KEY '+o.homeKey+'\n');
  fs.writeFileSync(path.join(app,'options.json'),JSON.stringify({nativeUid:o.nativeUid,sharedGid:o.sharedGid,model:o.model,firmware:o.firmware}));
  for(const f of ['LICENSE','UPSTREAM-LICENSE','PROVENANCE.md'])fs.copyFileSync(path.join(ROOT,f),path.join(app,f));
  fs.writeFileSync(path.join(out,'verified-options.json'),JSON.stringify(o));
  return {app,native,service:mode==='helper'?service:null};
}
function build(mode,config,out,opts={}){
  const o=validate(readJson(config),mode),receipt=mode==='helper'?readJson(opts.probe):null;
  if(mode==='helper')checkProbe(o,receipt);
  let zig,pack,python;
  if(opts.compile){zig=tool('ZIG');if(run(zig,['version'])!=='0.14.1')throw Error('Zig 0.14.1 required');}
  if(opts.package){if(!opts.compile)throw Error('Packaging requires explicit --compile');pack=tool('ARES_PACKAGE');python=tool('PYTHON');}
  const staged=stage(mode,o,out,receipt);
  if(opts.compile){
    for(const name of mode==='probe'?['detach']:['detach','relay']){
      const args=['cc','-target',o.target,'-std=c99','-D_GNU_SOURCE','-Os','-static','-s',path.join(staged.native,mode==='probe'?'identity.c':name+'.c')];
      if(name==='relay')args.push(path.join(staged.native,'filter.c'));args.push('-o',path.join(staged.app,name));run(zig,args);
    }
  }
  if(opts.package){
    run(pack,['--no-minify',staged.app,...(staged.service?[staged.service]:[]),'-o',out]);
    const files=fs.readdirSync(out).filter(n=>n.startsWith(APP+'_')&&n.endsWith('.ipk'));if(files.length!==1)throw Error('Expected one helper IPK');
    run(python,[path.join(ROOT,'fix-package.py'),path.join(out,files[0]),path.join(out,'verified-options.json')]);
    fs.writeFileSync(path.join(out,'SHA256SUMS'),crypto.createHash('sha256').update(fs.readFileSync(path.join(out,files[0]))).digest('hex')+'  '+files[0]+'\n');
  }
  return {mode,out,compiled:!!opts.compile,packaged:!!opts.package,installed:false,enabled:false};
}
module.exports={validate,checkProbe,stage,build};
if(require.main===module){try{
  const [mode,config,out,...flags]=process.argv.slice(2);let opts={};
  while(flags.length){const f=flags.shift();if(f==='--compile')opts.compile=true;else if(f==='--package')opts.package=true;else if(f==='--probe-receipt')opts.probe=flags.shift();else throw Error('Unknown option');}
  console.log(JSON.stringify(build(mode,config,out,opts)));
}catch(e){console.error(e.message);process.exitCode=1;}}
