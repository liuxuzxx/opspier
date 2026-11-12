#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';

const action=process.argv[2];
if(!['--check','--bump'].includes(action)) {
 console.error('Usage: node scripts/version.mjs --check | --bump');process.exit(1);
}
try {
 const packagePath='package.json',lockPath='package-lock.json',tauriPath='src-tauri/tauri.conf.json';
 const cargoPath='src-tauri/Cargo.toml',cargoLockPath='src-tauri/Cargo.lock';
 const readJson=path=>JSON.parse(readFileSync(path,'utf8'));
 const pkg=readJson(packagePath),lock=readJson(lockPath),tauri=readJson(tauriPath);
 const cargo=readFileSync(cargoPath,'utf8'),cargoLock=readFileSync(cargoLockPath,'utf8');
 const packageVersion=/(\[package\][\s\S]*?\nversion\s*=\s*")([^"]+)(")/;
 const lockedVersion=/(\[\[package\]\]\nname = "opspier"\nversion = ")([^"]+)(")/;
 const current=pkg.version;
 if(!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(current))throw Error('Expected a major.minor.patch version');
 const versions={package:current,packageLock:lock.version,packageLockRoot:lock.packages?.['']?.version,tauri:tauri.version,cargo:cargo.match(packageVersion)?.[2],cargoLock:cargoLock.match(lockedVersion)?.[2]};
 const mismatch=Object.entries(versions).filter(([,version])=>version!==current);
 if(mismatch.length)throw Error('Version mismatch: '+mismatch.map(([file,value])=>`${file}=${value}`).join(', '));
 if(action==='--bump') {
  const [major,minor,patch]=current.split('.').map(Number);
  const next=`${major}.${minor}.${patch+1}`;
  pkg.version=lock.version=lock.packages[''].version=tauri.version=next;
  for(const [path,value] of [[packagePath,pkg],[lockPath,lock],[tauriPath,tauri]])writeFileSync(path,JSON.stringify(value,null,2)+'\n');
  writeFileSync(cargoPath,cargo.replace(packageVersion,(_all,prefix,_old,suffix)=>prefix+next+suffix));
  writeFileSync(cargoLockPath,cargoLock.replace(lockedVersion,(_all,prefix,_old,suffix)=>prefix+next+suffix));
  console.log(`OpsPier delivery version ${current} -> ${next}`);
 } else console.log(`OpsPier ${current}: versions synchronized`);
} catch(error) {console.error(error.message);process.exit(1);}
