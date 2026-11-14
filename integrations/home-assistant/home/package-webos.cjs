#!/usr/bin/env node
'use strict';
// SPDX-License-Identifier: GPL-3.0-or-later
const fs=require('node:fs'),path=require('node:path'),crypto=require('node:crypto');
const {spawnSync}=require('node:child_process');

// Admit bounded regular files without following a leaf or parent symlink.
// Verify the same descriptor and directory identities after the bounded read.
function directoryChain(directory) {
  const absolute = path.resolve(directory);
  let current = path.parse(absolute).root;
  const rows = [];
  for (const part of ['', ...absolute.slice(current.length).split(path.sep).filter(Boolean)]) {
    if (part) current = path.join(current, part);
    const stat = fs.lstatSync(current);
    if (stat.isSymbolicLink() || !stat.isDirectory()) throw new Error('invalid_asset');
    rows.push({file:current,dev:stat.dev,ino:stat.ino});
  }
  return rows;
}
function verifyDirectories(rows) {
  for (const row of rows) {
    const stat = fs.lstatSync(row.file);
    if (stat.isSymbolicLink() || !stat.isDirectory() || stat.dev !== row.dev || stat.ino !== row.ino)
      throw new Error('invalid_asset');
  }
}
function readPublicFile(file, limit = 1024 * 1024) {
  const absolute = path.resolve(file);
  const parents = directoryChain(path.dirname(absolute));
  const before = fs.lstatSync(absolute);
  if (before.isSymbolicLink() || !before.isFile() || before.nlink !== 1 || before.size > limit) throw new Error('invalid_asset');
  const flags = fs.constants.O_RDONLY | (fs.constants.O_NOFOLLOW || 0) | (fs.constants.O_NONBLOCK || 0);
  const fd = fs.openSync(absolute, flags);
  try {
    const initial = fs.fstatSync(fd);
    if (!initial.isFile() || initial.nlink !== 1 || initial.dev !== before.dev || initial.ino !== before.ino || initial.size !== before.size)
      throw new Error('invalid_asset');
    const bytes = Buffer.alloc(initial.size + 1);
    let count = 0, read;
    while (count < bytes.length && (read = fs.readSync(fd, bytes, count, bytes.length - count, null)) > 0) count += read;
    const after = fs.fstatSync(fd), leaf = fs.lstatSync(absolute);
    if (count !== initial.size || !after.isFile() || after.nlink !== 1 || after.dev !== initial.dev || after.ino !== initial.ino ||
        after.size !== initial.size || after.mtimeMs !== initial.mtimeMs || after.ctimeMs !== initial.ctimeMs ||
        leaf.isSymbolicLink() || !leaf.isFile() || leaf.nlink !== 1 || leaf.dev !== initial.dev || leaf.ino !== initial.ino)
      throw new Error('invalid_asset');
    verifyDirectories(parents);
    return bytes.subarray(0, count);
  } finally { fs.closeSync(fd); }
}
const APP='com.openottplay.home', SERVICE=APP+'.network';
const ASSETS=['index.html','style.css','core.js','webos-host.js','app.js'];
const SERVICE_FILES=['service.js','probe.js','services.json','package.json','network.json','LICENSE','PROVENANCE.md'];
function fail(){throw new Error('Invalid home package inputs');}
function stage(options){
  if(!options||typeof options.out!=='string'||!options.out)fail();
  try { fs.lstatSync(options.out); fail(); } catch(error) { if(error.code!=='ENOENT')throw error; }
  const outputParents=directoryChain(path.dirname(path.resolve(options.out)));
  const dashboard=path.resolve(options.dashboard||path.join(__dirname,'dashboard'));
  const icon=path.resolve(options.icon||path.join(__dirname,'icon.png'));
  const service=options.service&&path.resolve(options.service);
  const assets=ASSETS.map(name=>({name,bytes:readPublicFile(path.join(dashboard,name))}));
  for(const asset of assets)if(asset.bytes.length===0||asset.bytes.length>1024*1024)fail();
  const png=readPublicFile(icon);if(png.length>1024*1024||png.subarray(0,8).toString('hex')!=='89504e470d0a1a0a')fail();
  let serviceFiles;
  if(service){
    serviceFiles=SERVICE_FILES.map(name=>({name,bytes:readPublicFile(path.join(service,name))}));
    const metadata=JSON.parse(serviceFiles.find(x=>x.name==='services.json').bytes);
    const pkg=JSON.parse(serviceFiles.find(x=>x.name==='package.json').bytes);
    if(metadata.id!==SERVICE||metadata.services?.length!==1||metadata.services[0].name!==SERVICE||pkg.name!==SERVICE||pkg.main!=='service.js')fail();
    const network=serviceFiles.find(x=>x.name==='network.json');
    const config=JSON.parse(network.bytes);
    if(!config||typeof config!=='object'||Array.isArray(config))fail();
    config.enabled=options.enableNetwork===true;
    network.bytes=Buffer.from(JSON.stringify(config,null,2)+'\n');
  }else if(options.enableNetwork)fail();
  const root=path.resolve(options.out),app=path.join(root,'app');
  verifyDirectories(outputParents);fs.mkdirSync(root,{recursive:false});
  const stageRoot=fs.lstatSync(root);
  function verifyStage(){
    verifyDirectories(outputParents);
    const stat=fs.lstatSync(root);
    if(stat.isSymbolicLink()||!stat.isDirectory()||stat.dev!==stageRoot.dev||stat.ino!==stageRoot.ino)fail();
  }
  function writeNew(file,bytes){
    verifyStage();
    const parents=directoryChain(path.dirname(file));
    const flags=fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|(fs.constants.O_NOFOLLOW||0);
    const fd=fs.openSync(file,flags,0o644);
    try {
      const owned=fs.fstatSync(fd);if(!owned.isFile())fail();
      fs.writeFileSync(fd,bytes);
      const leaf=fs.lstatSync(file);
      if(leaf.isSymbolicLink()||!leaf.isFile()||leaf.dev!==owned.dev||leaf.ino!==owned.ino)fail();
      verifyDirectories(parents);verifyStage();
    } finally {fs.closeSync(fd);}
  }
  verifyStage();fs.mkdirSync(app);
  for(const asset of assets)writeNew(path.join(app,asset.name),asset.bytes);
  writeNew(path.join(app,'icon.png'),png);
  writeNew(path.join(app,'appinfo.json'),JSON.stringify({id:APP,version:'1.0.0',vendor:'OTTPlay',title:'OTTPlay Home',
    type:'web',main:'index.html',icon:'icon.png',appDescription:'Optional home controls and manual network diagnostics',
    requiredPermissions:service?['networkconnection.query']:[],
    requiredACG:service?['network.query','application.launcher']:['application.launcher']},null,2)+'\n');
  let serviceDir;
  if(serviceFiles){
    serviceDir=path.join(root,'network-service');verifyStage();fs.mkdirSync(serviceDir);
    for(const file of serviceFiles)writeNew(path.join(serviceDir,file.name),file.bytes);

  }
  const files=[];
  function inventory(dir){for(const item of fs.readdirSync(dir,{withFileTypes:true})){const file=path.join(dir,item.name);if(item.isDirectory())inventory(file);else files.push({path:path.relative(root,file),sha256:crypto.createHash('sha256').update(readPublicFile(file)).digest('hex')});}}
  inventory(root);
  writeNew(path.join(root,'manifest.json'),JSON.stringify({app:APP,service:service?SERVICE:null,networkEnabled:options.enableNetwork===true,files},null,2)+'\n');
  return{root,app,service:serviceDir};
}
function main(args){
  const options={};let pack=false;
  for(let i=0;i<args.length;i++){
    const key=args[i];if(key==='--package'){pack=true;continue;}if(key==='--enable-network-probes'){options.enableNetwork=true;continue;}
    const fields={'--out':'out','--dashboard':'dashboard','--icon':'icon','--network-service':'service'};
    if(!fields[key]||!args[i+1]||args[i+1].startsWith('--'))fail();options[fields[key]]=args[++i];
  }
  const result=stage(options);
  if(pack){
    const destination=path.join(result.root,'ipk');fs.mkdirSync(destination);
    const inputs=[result.app,...(result.service?[result.service]:[])];
    const run=spawnSync('ares-package',['--no-minify',...inputs,'-o',destination],{stdio:'inherit',timeout:120000,shell:false});
    if(run.error||run.status!==0)throw new Error('webOS packaging failed; staged source retained');
  }
  console.log(result.root);
}
if(require.main===module){try{main(process.argv.slice(2));}catch{console.error('OTT home packaging failed; inspect inputs and retained stage');process.exitCode=1;}}
module.exports={stage};
