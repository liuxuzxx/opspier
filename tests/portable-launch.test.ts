import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,copyFileSync,writeFileSync,chmodSync,readFileSync,rmSync} from 'node:fs';
import {join,resolve} from 'node:path';
import {tmpdir} from 'node:os';
import {spawnSync} from 'node:child_process';

// Catches launch scripts that depend on the original checkout or silently replace caller overrides.
test('a relocated checkout launches its built binary from the user cache and forwards arguments',()=>{
 const root=mkdtempSync(join(tmpdir(),'opspier relocated '));
 try {
  const project=join(root,'checkout with spaces'),cache=join(root,'cache'),home=join(root,'home');
  for(const p of [project,home,join(cache,'opspier/target/release')])mkdirSync(p,{recursive:true});
  for(const name of ['build-env.sh','run.sh'])copyFileSync(resolve(name),join(project,name));
  const binary=join(cache,'opspier/target/release/opspier'),record=join(root,'record');
  writeFileSync(binary,'#!/bin/sh\nprintf "%s\\n" "$@" > "$LAUNCH_RECORD"\n');chmodSync(binary,0o755);
  const run=spawnSync('bash',[join(project,'run.sh'),'argument with spaces','--read-only'],{cwd:home,env:{HOME:home,PATH:'/usr/bin:/bin',XDG_CACHE_HOME:cache,LAUNCH_RECORD:record},encoding:'utf8'});
  assert.equal(run.status,0,run.stderr);assert.equal(readFileSync(record,'utf8'),'argument with spaces\n--read-only\n');
 } finally {rmSync(root,{recursive:true,force:true});}
});
test('an explicit installed binary and native build overrides survive relocation',()=>{
 const root=mkdtempSync(join(tmpdir(),'opspier-overrides-'));
 try {
  const binary=join(root,'installed binary'),record=join(root,'record');
  writeFileSync(binary,'#!/bin/sh\nprintf "%s|%s|%s" "$CARGO_TARGET_DIR" "$CARGO_BUILD_JOBS" "${PKG_CONFIG_SYSROOT_DIR-unset}" > "$LAUNCH_RECORD"\n');chmodSync(binary,0o755);
  const run=spawnSync('bash',[resolve('run.sh')],{env:{HOME:root,PATH:'/usr/bin:/bin',OPSPIER_APPLICATION:binary,CARGO_TARGET_DIR:join(root,'target'),CARGO_BUILD_JOBS:'1',LAUNCH_RECORD:record},encoding:'utf8'});
  assert.equal(run.status,0,run.stderr);assert.equal(readFileSync(record,'utf8'),join(root,'target')+'|1|unset');
 } finally {rmSync(root,{recursive:true,force:true});}
});
