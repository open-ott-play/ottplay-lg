'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),os=require('node:os'),path=require('node:path');
const {stage}=require('./package-webos.cjs');
function fixture(fn){const root=fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()),'ott-home-package-'));try{fn(root);}finally{fs.rmSync(root,{recursive:true,force:true});}}
test('only declared credential-free dashboard assets enter stage, never adjacent private files',()=>fixture(root=>{
  const src=path.join(root,'assets');fs.mkdirSync(src);
  for(const file of ['index.html','style.css','core.js','webos-host.js','app.js'])fs.writeFileSync(path.join(src,file),'/* fixture */');
  fs.writeFileSync(path.join(src,'config.private.json'),'SECRET');
  const out=path.join(root,'stage'),result=stage({dashboard:src,out});
  const files=fs.readdirSync(result.app);assert.ok(!files.includes('config.private.json'));
  const info=JSON.parse(fs.readFileSync(path.join(result.app,'appinfo.json')));assert.equal(info.id,'com.openottplay.home');
  assert.deepEqual(info.requiredPermissions,[]);assert.equal(result.service,undefined);
  assert.throws(()=>stage({dashboard:src,out}),'existing stage must not be overwritten');
}));
test('service identity and explicit network opt-in are verified before staging',()=>fixture(root=>{
  const base=fs.existsSync(path.join(__dirname,'dashboard'))?__dirname:path.join(__dirname,'..');
  const service=path.join(base,'network-service');
  const dashboard=path.join(base,'dashboard');
  const result=stage({out:path.join(root,'stage'),dashboard,service});
  assert.equal(JSON.parse(fs.readFileSync(path.join(result.service,'network.json'))).enabled,false);
  const enabled=stage({out:path.join(root,'enabled'),dashboard,service,enableNetwork:true});
  assert.equal(JSON.parse(fs.readFileSync(path.join(enabled.service,'network.json'))).enabled,true);
  assert.throws(()=>stage({out:path.join(root,'bad'),dashboard,enableNetwork:true}));
  assert.ok(!fs.existsSync(path.join(root,'bad')));
}));

test('fixed inputs reject leaf and parent symlinks, directories and oversized service files before staging',()=>fixture(root=>{
  const src=path.join(root,'assets');fs.mkdirSync(src);
  for(const file of ['index.html','style.css','core.js','webos-host.js','app.js'])fs.writeFileSync(path.join(src,file),'public');
  const secret=path.join(root,'private.json');fs.writeFileSync(secret,'PRIVATE_SENTINEL',{mode:0o600});
  fs.unlinkSync(path.join(src,'app.js'));fs.symlinkSync(secret,path.join(src,'app.js'));
  const leaf=path.join(root,'leaf');assert.throws(()=>stage({dashboard:src,out:leaf}));assert.ok(!fs.existsSync(leaf));
  fs.unlinkSync(path.join(src,'app.js'));fs.linkSync(secret,path.join(src,'app.js'));
  assert.throws(()=>stage({dashboard:src,out:path.join(root,'hardlink-stage')}));
  assert.equal(fs.readFileSync(secret,'utf8'),'PRIVATE_SENTINEL');
  fs.unlinkSync(path.join(src,'app.js'));fs.writeFileSync(path.join(src,'app.js'),'public');
  const alias=path.join(root,'alias');fs.symlinkSync(src,alias,'dir');
  const parent=path.join(root,'parent');assert.throws(()=>stage({dashboard:alias,out:parent}));assert.ok(!fs.existsSync(parent));
  fs.unlinkSync(path.join(src,'app.js'));fs.mkdirSync(path.join(src,'app.js'));
  assert.throws(()=>stage({dashboard:src,out:path.join(root,'directory')}));
  fs.rmdirSync(path.join(src,'app.js'));fs.writeFileSync(path.join(src,'app.js'),'public');
  const icon=path.join(root,'icon.png');fs.symlinkSync(path.join(__dirname,'icon.png'),icon);
  assert.throws(()=>stage({dashboard:src,icon,out:path.join(root,'icon-stage')}));
  const base=fs.existsSync(path.join(__dirname,'dashboard'))?__dirname:path.join(__dirname,'..');
  const service=path.join(root,'service');fs.cpSync(path.join(base,'network-service'),service,{recursive:true});
  fs.unlinkSync(path.join(service,'network.json'));fs.symlinkSync(secret,path.join(service,'network.json'));
  assert.throws(()=>stage({dashboard:src,service,out:path.join(root,'service-symlink')}));
  fs.unlinkSync(path.join(service,'network.json'));fs.writeFileSync(path.join(service,'network.json'),'x'.repeat(1024*1024+1));
  assert.throws(()=>stage({dashboard:src,service,out:path.join(root,'service-oversized')}));
}));
test('output parent symlinks and dangling output entries cannot redirect a new stage',()=>fixture(root=>{
  const src=path.join(root,'assets');fs.mkdirSync(src);
  for(const file of ['index.html','style.css','core.js','webos-host.js','app.js'])fs.writeFileSync(path.join(src,file),'public');
  const real=path.join(root,'real');fs.mkdirSync(real);
  const alias=path.join(root,'alias');fs.symlinkSync(real,alias,'dir');
  assert.throws(()=>stage({dashboard:src,out:path.join(alias,'stage')}));
  assert.deepEqual(fs.readdirSync(real),[]);
  const dangling=path.join(root,'dangling');fs.symlinkSync(path.join(root,'absent'),dangling);
  assert.throws(()=>stage({dashboard:src,out:dangling}));
  assert.ok(fs.lstatSync(dangling).isSymbolicLink());
}));

test('input replacement between path admission and descriptor open cannot package private bytes',()=>fixture(root=>{
  for(const mode of ['leaf','parent']){
    const src=path.join(root,mode);fs.mkdirSync(src);
    for(const name of ['index.html','style.css','core.js','webos-host.js','app.js'])fs.writeFileSync(path.join(src,name),'public');
    const other=path.join(root,mode+'-private');fs.mkdirSync(other);
    fs.writeFileSync(path.join(other,'app.js'),'PRIVATE_SENTINEL',{mode:0o600});
    const target=path.join(src,'app.js'),out=path.join(root,mode+'-stage');
    const originalOpen=fs.openSync;let swapped=false;
    try {
      fs.openSync=function(file,...args){
        if(file===target&&!swapped){
          swapped=true;
          if(mode==='leaf'){
            fs.unlinkSync(target);fs.renameSync(path.join(other,'app.js'),target);
          } else {
            fs.renameSync(src,src+'-original');fs.symlinkSync(other,src,'dir');
          }
        }
        return originalOpen.call(fs,file,...args);
      };
      assert.throws(()=>stage({dashboard:src,out}));
      assert.equal(swapped,true);assert.equal(fs.existsSync(out),false);
    } finally {fs.openSync=originalOpen;}
  }
}));
