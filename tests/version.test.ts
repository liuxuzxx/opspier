import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

function fixture(run:(root:string,invoke:(arg:string)=>ReturnType<typeof spawnSync>)=>void) {
 const root=mkdtempSync(join(tmpdir(),'ops-version-'));mkdirSync(join(root,'src-tauri'));
 const write=(name:string,value:unknown)=>writeFileSync(join(root,name),JSON.stringify(value));
 write('package.json',{name:'opspier',version:'0.1.0'});
 write('package-lock.json',{name:'opspier',version:'0.1.0',packages:{'':{name:'opspier',version:'0.1.0'},'node_modules/other':{version:'9.9.9'}}});
 write('src-tauri/tauri.conf.json',{productName:'OpsPier',version:'0.1.0'});
 writeFileSync(join(root,'src-tauri/Cargo.toml'),'[package]\nname = "opspier"\nversion = "0.1.0"\n');
 writeFileSync(join(root,'src-tauri/Cargo.lock'),'version = 4\n\n[[package]]\nname = "opspier"\nversion = "0.1.0"\n\n[[package]]\nname = "other"\nversion = "9.9.9"\n');
 try {run(root,arg=>spawnSync(process.execPath,[fileURLToPath(new URL('../scripts/version.mjs',import.meta.url)),arg],{cwd:root,encoding:'utf8'}));}
 finally {rmSync(root,{recursive:true,force:true});}
}
test('one delivery patch bump synchronizes package, Cargo, Tauri and locks without changing dependencies',()=>fixture((root,invoke)=>{
 assert.equal(invoke('--check').status,0);
 assert.equal(invoke('--bump').status,0);
 for(const file of ['package.json','package-lock.json','src-tauri/tauri.conf.json'])assert.equal(JSON.parse(readFileSync(join(root,file),'utf8')).version,'0.1.1');
 const lock=JSON.parse(readFileSync(join(root,'package-lock.json'),'utf8'));assert.equal(lock.packages[''].version,'0.1.1');assert.equal(lock.packages['node_modules/other'].version,'9.9.9');
 assert.match(readFileSync(join(root,'src-tauri/Cargo.toml'),'utf8'),/version = "0\.1\.1"/);
 assert.match(readFileSync(join(root,'src-tauri/Cargo.lock'),'utf8'),/name = "opspier"\nversion = "0\.1\.1"/);
 assert.match(readFileSync(join(root,'src-tauri/Cargo.lock'),'utf8'),/name = "other"\nversion = "9\.9\.9"/);
 assert.equal(invoke('--check').status,0);
 assert.equal(JSON.parse(readFileSync(join(root,'package.json'),'utf8')).version,'0.1.1');
}));
test('version check rejects a mismatch and does not silently change any version',()=>fixture((root,invoke)=>{
 writeFileSync(join(root,'src-tauri/tauri.conf.json'),JSON.stringify({version:'0.1.8'}));
 const result=invoke('--check');assert.equal(result.status,1);assert.match(String(result.stderr),/mismatch/i);
 assert.equal(JSON.parse(readFileSync(join(root,'src-tauri/tauri.conf.json'),'utf8')).version,'0.1.8');
 assert.equal(JSON.parse(readFileSync(join(root,'package.json'),'utf8')).version,'0.1.0');
}));
