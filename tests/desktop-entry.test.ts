import {test} from 'node:test';
import assert from 'node:assert/strict';
import {mkdtempSync,mkdirSync,readFileSync,rmSync,writeFileSync,chmodSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {execFileSync} from 'node:child_process';
test('installer creates only OpsPier launchers in an isolated desktop and app-menu',()=>{
 const root=mkdtempSync(join(tmpdir(),'ops-desktop-entry-'));
 try{
  const home=join(root,'home');const bin=join(root,'bin');mkdirSync(home);mkdirSync(bin);
  const desktop=join(home,'Desktop');mkdirSync(desktop);
  const stub=join(bin,'xdg-user-dir');writeFileSync(stub,'#!/bin/sh\nprintf "%s\\n" "$OPS_TEST_DESKTOP"\n');chmodSync(stub,0o755);
  const script=resolve('scripts/install-desktop.sh');
  execFileSync('bash',[script],{env:{PATH:bin+':/usr/bin:/bin',HOME:home,XDG_DATA_HOME:join(home,'data'),OPS_TEST_DESKTOP:desktop}});
  const app=readFileSync(join(home,'data/applications/com.aic.ops.desktop'),'utf8');assert.equal(app,readFileSync(join(desktop,'com.aic.ops.desktop'),'utf8'));
  assert.ok(app.includes('Name=OpsPier\n'));assert.ok(app.includes('Terminal=false\n'));assert.ok(app.includes('/run.sh"\n'));assert.ok(app.includes('com.aic.ops.png\n'));assert.ok(app.includes('StartupWMClass=opspier\n'));assert.ok(!app.includes('password'));
 }finally{rmSync(root,{recursive:true,force:true});}
});

// Catches machine-specific runtime overrides being lost when launching from the desktop.
test('desktop launcher forwards only explicit binary and sysroot overrides without shell evaluation',()=>{
 const root=mkdtempSync(join(tmpdir(),'opspier desktop '));
 try {
  const home=join(root,'home'),bin=join(root,'bin'),desktop=join(home,'Desktop');mkdirSync(desktop,{recursive:true});mkdirSync(bin);
  const stub=join(bin,'xdg-user-dir');writeFileSync(stub,'#!/bin/sh\nprintf "%s\\n" "$OPS_TEST_DESKTOP"\n');chmodSync(stub,0o755);
  const application=join(root,'installed binary'),sysroot=join(root,'system libraries');
  execFileSync('bash',[resolve('scripts/install-desktop.sh')],{env:{PATH:bin+':/usr/bin:/bin',HOME:home,XDG_DATA_HOME:join(home,'data'),OPS_TEST_DESKTOP:desktop,OPSPIER_APPLICATION:application,OPSPIER_SYSROOT:sysroot,UNRELATED_TOKEN:'synthetic-not-for-launcher'}});
  const app=readFileSync(join(desktop,'com.aic.ops.desktop'),'utf8');
  assert.ok(app.includes('"OPSPIER_APPLICATION='+application+'"'));assert.ok(app.includes('"OPSPIER_SYSROOT='+sysroot+'"'));
  assert.ok(app.includes('Exec="/usr/bin/env" '));assert.ok(!app.includes('synthetic-not-for-launcher'));
 }finally{rmSync(root,{recursive:true,force:true});}
});

// Exercises the actual freedesktop parser, including its string-decoding and Exec-decoding layers.
test('Gio launches a desktop entry whose runtime paths contain reserved characters',{skip:process.platform!=='linux'},()=>{
 const root=mkdtempSync(join(tmpdir(),'opspier-gio-'));
 try {
  const home=join(root,'home'),bin=join(root,'bin'),desktop=join(home,'Desktop');mkdirSync(desktop,{recursive:true});mkdirSync(bin);
  const stub=join(bin,'xdg-user-dir');writeFileSync(stub,'#!/bin/sh\nprintf "%s\\n" "$OPS_TEST_DESKTOP"\n');chmodSync(stub,0o755);
  const application=join(root,'installed $ " ` \\ % binary'),sysroot=join(root,'libraries $ " ` \\ %'),record=join(root,'launch-record');
  writeFileSync(application,'#!/bin/sh\nprintf "%s" "$OPSPIER_SYSROOT" > "$LAUNCH_RECORD"\n');chmodSync(application,0o755);
  const env={PATH:bin+':/usr/bin:/bin',HOME:home,XDG_DATA_HOME:join(home,'data'),OPS_TEST_DESKTOP:desktop,OPSPIER_APPLICATION:application,OPSPIER_SYSROOT:sysroot,LAUNCH_RECORD:record};
  execFileSync('bash',[resolve('scripts/install-desktop.sh')],{env});
  execFileSync('/usr/bin/python3',['-c',`
import ctypes,ctypes.util,pathlib,sys,time
lib=ctypes.CDLL(ctypes.util.find_library('gio-2.0'))
lib.g_desktop_app_info_new_from_filename.argtypes=[ctypes.c_char_p];lib.g_desktop_app_info_new_from_filename.restype=ctypes.c_void_p
lib.g_app_info_get_commandline.argtypes=[ctypes.c_void_p];lib.g_app_info_get_commandline.restype=ctypes.c_char_p
lib.g_app_info_launch.argtypes=[ctypes.c_void_p,ctypes.c_void_p,ctypes.c_void_p,ctypes.POINTER(ctypes.c_void_p)];lib.g_app_info_launch.restype=ctypes.c_int
app=lib.g_desktop_app_info_new_from_filename(sys.argv[1].encode());assert app,'Gio rejected desktop entry'
assert lib.g_app_info_get_commandline(app),'Gio rejected command line'
error=ctypes.c_void_p();assert lib.g_app_info_launch(app,None,None,ctypes.byref(error)),'Gio failed to launch'
record=pathlib.Path(sys.argv[2]);deadline=time.monotonic()+5
while not record.exists() and time.monotonic()<deadline:time.sleep(.05)
assert record.exists(),'No application launch record'
`,join(desktop,'com.aic.ops.desktop'),record],{env});
  assert.equal(readFileSync(record,'utf8'),sysroot);
 }finally{rmSync(root,{recursive:true,force:true});}
});
