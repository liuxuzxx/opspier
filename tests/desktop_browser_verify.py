#!/usr/bin/env python3
"""OpsPier 0.1.7 actual app/Secret Service persistence, UI and native entry checks.

Only synthetic loopback services and a private DBus/HOME/X196 are used. Secrets
are passed in memory/stdin and never serialized into evidence or exception text.
"""
import argparse
import base64
import csv
import hashlib
from http.client import RemoteDisconnected
import io
import json
import os
import re
from pathlib import Path
import secrets
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback
sys.path.insert(0,str(Path(__file__).parent))
from desktop_driver import Driver,free_port,ELEMENT
sys.path.insert(0,str(Path(__file__).parent/'fixtures'))
from desktop_services import start,start_tls_mysql
ROOT=Path(__file__).resolve().parents[1]
CACHE=Path(os.environ.get('OPSPIER_TEST_CACHE',str(Path.home()/'.cache/opspier')))
XDOTOOL=os.environ.get('OPSPIER_XDOTOOL','xdotool')
XVFB=os.environ.get('OPSPIER_XVFB','Xvfb')


class StableDriver(Driver):
    """Retry transport loss only while polling read-only UI conditions, never actions."""
    read_retries=0
    def wait(self,script,*args,timeout=12):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            try:
                result=self.js(script,*args)
                if result:return result
            except (RemoteDisconnected,ConnectionResetError):
                self.read_retries+=1
            time.sleep(.1)
        raise AssertionError('read-only UI condition timed out')


def inside(output,application,previous_application=None):
    private=Path(os.environ['HOME']);processes=[];logs=[];daemon=None;driver=None;driver_process=None;db=kube=tls_db=tls_linger=None
    report={'status':'running','scope':'actual release, private HOME/XDG/DBus/real GNOME keyring, loopback services','ui':[],'ipc':[],'measurements':{},'appRuns':[]}
    phase='startup';owned=[];login=secrets.token_hex(36).encode();password='db-'+secrets.token_hex(32);ssh_password='ssh-'+secrets.token_hex(32);token='kube-'+secrets.token_hex(32)
    def check(name,layer='ui'):
        report[layer].append(name);print('PASS '+layer+': '+name,flush=True)
    def launch(args,logname,env=None,stdin=None):
        log=open(output/logname,'wb');logs.append(log)
        proc=subprocess.Popen(args,env=env,cwd=private,stdin=stdin,stdout=log,stderr=log,start_new_session=True);processes.append(proc);return proc
    def secret_bus_owned():
        result=subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.NameHasOwner','org.freedesktop.secrets'],capture_output=True,timeout=3)
        return result.returncode==0 and b'true' in result.stdout
    def start_keyring():
        proc=subprocess.Popen(['gnome-keyring-daemon','--foreground','--unlock','--components=secrets','--control-directory',str(private/'run/keyring')],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
        processes.append(proc);proc.stdin.write(login+b'\n');proc.stdin.close()
        deadline=time.monotonic()+12
        while time.monotonic()<deadline:
            if proc.poll() is not None:raise RuntimeError('private credential daemon exited')
            if secret_bus_owned():return proc
            time.sleep(.1)
        raise RuntimeError('private credential daemon unavailable')
    def stop_keyring():
        nonlocal daemon
        if daemon:
            daemon.terminate();daemon.wait(timeout=5);daemon=None
        deadline=time.monotonic()+6
        while secret_bus_owned():
            if time.monotonic()>deadline:raise RuntimeError('private daemon did not release bus')
            time.sleep(.1)
    def app_pids():
        rows=subprocess.check_output(['ps','-eo','pid,pgid,comm'],text=True).splitlines()[1:]
        return [int(row.split()[0]) for row in rows if len(row.split())>=3 and int(row.split()[1])==driver_process.pid and row.split()[2] in ('opspier','aic-ops')]
    def start_app(label):
        nonlocal driver
        active=previous_application if label=='A-save' and previous_application else application
        brand,version=('OPS','0.1.4') if active==previous_application else ('OpsPier','0.1.7')
        session=driver.call('POST','/session',{'capabilities':{'alwaysMatch':{'browserName':'wry','tauri:options':{'application':str(active)}}}});driver.sid=session['sessionId']
        driver.cmd('POST','/timeouts',{'script':30000,'implicit':0,'pageLoad':30000});driver.cmd('POST','/window/rect',{'width':1460,'height':1060,'x':220,'y':10})
        driver.text('从左侧新建或打开连接');driver.text(f'{brand} {version}')
        driver.js("window.__opsPageErrors=[];window.addEventListener('error',e=>window.__opsPageErrors.push({name:e.error?.name||'Error',message:e.message||''}));window.addEventListener('unhandledrejection',e=>window.__opsPageErrors.push({name:e.reason?.name||'Rejection',message:typeof e.reason==='string'?e.reason:(e.reason?.message||'')}));")
        pids=app_pids();assert len(pids)==1,'one owned application process per app run';owned.extend(pids)
        actual=hashlib.sha256(Path(f'/proc/{pids[0]}/exe').read_bytes()).hexdigest();assert actual==hashlib.sha256(active.read_bytes()).hexdigest()
        report['appRuns'].append({'stage':label,'pid':pids[0],'sha256':actual,'brand':brand,'version':version})
        assert driver.js('return document.title')==f'{brand} {version}'
        windows=subprocess.check_output([XDOTOOL,'search','--all','--pid',str(pids[0]),'--name',brand],text=True).split()
        titles=[subprocess.check_output([XDOTOOL,'getwindowname',w],text=True).strip() for w in windows]
        assert f'{brand} · 运维工作台 {version}' in titles;report['nativeTitle']=f'{brand} · 运维工作台 {version}'
    def stop_app():
        driver.cmd('DELETE','');driver.sid=''
        deadline=time.monotonic()+10
        while app_pids():
            if time.monotonic()>deadline:raise RuntimeError('owned application did not exit')
            time.sleep(.1)
    def new_profile(kind):
        driver.select('.connection-sidebar .ant-select',kind)

        button=driver.element('.connection-sidebar button.ant-btn-primary')
        driver.cmd('POST',f'/element/{button[ELEMENT]}/click',{})
        driver.wait("return [...document.querySelectorAll('.ant-modal #name')].some(e=>e.getClientRects().length)")
    def profile_row(name):
        return driver.wait("return [...document.querySelectorAll('.profile-item')].find(e=>e.querySelector('.profile-name')?.textContent===arguments[0])",name)
    def profile_action(name,action):
        row=profile_row(name);driver.click(driver.wait("return [...arguments[0].querySelectorAll('button')].find(e=>e.textContent===arguments[1]&&!e.disabled)",row,action))
    def open_profile(name,wait_active=True):
        row=profile_row(name);driver.click(driver.js("return arguments[0].querySelector('.profile-name')",row))
        category=driver.js("return arguments[0].closest('.profile-category')?.querySelector('.category-heading')?.textContent",row)
        if wait_active and category!='主机与终端':driver.wait("return [...document.querySelectorAll('[role=tab][aria-selected=true]')].some(e=>e.textContent.includes(arguments[0]))||[...document.querySelectorAll('.ant-modal')].some(e=>e.getClientRects().length&&e.querySelector('.ant-modal-title')?.textContent==='选择要打开的 Context')",name)
    def saved_modal_closed():
        driver.wait("return [...document.querySelectorAll('.ant-modal')].every(e=>!e.getClientRects().length)")
    def fill_connection(kind,name,secret):
        new_profile(kind)
        for field,value in {'name':name,'host':'localhost','port':ssh_info['port'] if kind=='SSH' else db.server_address[1],'username':'fixture-user','password':secret}.items():driver.fill('.ant-modal #'+field,value)
    def select_resource(text):
        selector='.workspace:not([hidden]) [aria-label=资源类型]'
        element=driver.element(selector)
        driver.js("arguments[0].dispatchEvent(new MouseEvent('mousedown',{bubbles:true,button:0}));arguments[0].click()",element)
        driver.fill(selector,text)
        driver.click(driver.wait("return [...document.querySelectorAll('.ant-select-item-option')].find(e=>e.textContent.trim()===arguments[0]&&e.getClientRects().length)",text))
    def snapshot(name):driver.screenshot(output/name)
    def query_db():
        driver.text('8.0.36-desktop-fixture')
        driver.fill('.workspace:not([hidden]) textarea','SELECT * FROM demo.items')
        driver.button('执行只读查询')
        driver.text('250 行')
        driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='执行只读查询'&&!e.disabled)")
    def import_kube(name,mode,config_path=None,text=None,skip_inspect=False):
        new_profile('Kubernetes');driver.fill('.ant-modal #name',name)
        if mode=='file':
            driver.button('选择文件')
            deadline=time.monotonic()+8;chooser=[]
            while not chooser:
                result=subprocess.run([XDOTOOL,'search','--all','--onlyvisible','--name','选择 kubeconfig 文件'],capture_output=True,text=True)
                chooser=result.stdout.split()
                if time.monotonic()>deadline:raise AssertionError('native file chooser unavailable')
                time.sleep(.1)
            subprocess.run([XDOTOOL,'windowfocus','--sync',chooser[-1]],check=True)
            subprocess.run([XDOTOOL,'key','--clearmodifiers','ctrl+l'],check=True)
            subprocess.run([XDOTOOL,'type','--clearmodifiers',str(config_path)],check=True)
            subprocess.run([XDOTOOL,'key','Return'],check=True)
            time.sleep(.3)
            subprocess.run([XDOTOOL,'key','Return'],check=True)
            driver.wait("return document.querySelector('input[aria-label=kubeconfig文件路径]')?.value===arguments[0]",str(config_path))
        elif mode=='drag':
            helper=subprocess.Popen(['/usr/bin/python3',str(ROOT/'tests/fixtures/drag_source.py')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,start_new_session=True);processes.append(helper)
            helper.stdin.write(json.dumps({'path':str(config_path)}).encode()+b'\n');helper.stdin.flush();assert helper.stdout.readline().strip()==b'ready'
            deadline=time.monotonic()+5;windows=[]
            while not windows:
                result=subprocess.run([XDOTOOL,'search','--all','--onlyvisible','--pid',str(helper.pid),'--name','OPS fixture file drag'],capture_output=True,text=True)
                windows=result.stdout.split()
                if time.monotonic()>deadline:raise AssertionError('private drag source did not map')
                time.sleep(.1)
            raw=subprocess.check_output([XDOTOOL,'getwindowgeometry','--shell',windows[-1]],text=True)
            geometry={k:int(v) for k,v in (line.split('=',1) for line in raw.splitlines())}
            sx=geometry['X']+geometry['WIDTH']//2;sy=geometry['Y']+geometry['HEIGHT']//2
            bounds=driver.js("const r=document.querySelector('.kube-drop').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};")
            frame=driver.cmd('GET','/window/rect');tx=int(frame['x']+bounds['x']);ty=int(frame['y']+bounds['y'])
            subprocess.run([XDOTOOL,'mousemove','--sync',str(sx),str(sy),'mousedown','1'],check=True)
            time.sleep(.2)
            for part in [.05,.15,.35,.6,.8,1]:
                x=int(sx+(tx-sx)*part);y=int(sy+(ty-sy)*part)
                subprocess.run([XDOTOOL,'mousemove','--sync',str(x),str(y)],check=True);time.sleep(.2)
            time.sleep(.5);subprocess.run([XDOTOOL,'mouseup','1'],check=True);time.sleep(.4)
            events=[]
            while select.select([helper.stdout],[],[],.1)[0]:
                value=helper.stdout.readline().decode().strip()
                if value in ['begin','end','data','failed']:events.append(value)
                else:break
            report['measurements']['nativeDragEvents']=events
            driver.wait("return document.querySelector('input[aria-label=kubeconfig文件路径]')?.value===arguments[0]",str(config_path))
            helper.terminate();helper.wait(timeout=3)
        else:
            driver.select('.ant-modal .ant-select','粘贴配置文本');driver.fill('textarea[aria-label=kubeconfig配置文本]',text)
        if not skip_inspect:
            driver.button('解析配置');driver.text('2 个 Context');driver.select('.ant-modal #context','local')
        driver.button('保存集群');saved_modal_closed();profile_row(name)
    def reopen_all(stage):
        before_db=db.auth_success;before_kube=kube.auth_success
        open_profile('SSH已保存');driver.wait("return document.querySelector('[data-testid=ssh-status]')?.textContent==='已连接'")
        assert driver.js("return [...document.querySelectorAll('.ant-modal')].every(e=>!e.getClientRects().length)")
        open_profile('MySQL已保存');query_db();assert db.auth_success>before_db
        open_profile('Doris已保存');query_db()
        for name in ['文件集群','拖入集群','粘贴集群']:
            open_profile(name)
            if driver.js("return [...document.querySelectorAll('.ant-modal')].some(e=>e.getClientRects().length&&e.querySelector('.ant-modal-title')?.textContent==='选择要打开的 Context')"):
                driver.select('.ant-modal .ant-select','local');driver.button('打开集群')
                driver.wait("return [...document.querySelectorAll('.workspace-tabs [role=tab][aria-selected=true]')].some(e=>e.textContent.includes(arguments[0]))",name)
            driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='api')")
        assert kube.auth_success>before_kube
        saved_modal_closed()
        check(stage+'-all-four-drivers-reopen-without-credential-entry')
    try:
        if Path('/tmp/.X11-unix/X196').exists() or Path('/tmp/.X196-lock').exists():raise RuntimeError('private display occupied')
        launch([XVFB,':196','-screen','0','1900x1200x24','-nolisten','tcp','-ac'],'xvfb.log')
        for _ in range(60):
            if Path('/tmp/.X11-unix/X196').exists():break
            time.sleep(.1)
        daemon=start_keyring();db,kube=start(password,token)
        ssh_env={**os.environ,'PYTHONPATH':os.environ.get('AIC_OPS_FIXTURE_PYTHON_PATH',''),'AIC_OPS_FIXTURE_TRACE':str(private/'ssh-trace.jsonl')}
        ssh=subprocess.Popen(['/usr/bin/python3',str(ROOT/'tests/fixtures/ssh_server.py'),'--password-stdin'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env=ssh_env,start_new_session=True);processes.append(ssh)
        ssh.stdin.write(json.dumps({'password':ssh_password}).encode()+b'\n');ssh.stdin.flush();ssh_info=json.loads(ssh.stdout.readline())
        config_dir=private/'input-fixture';config_dir.mkdir();(config_dir/'token').write_text(token)
        source=f"apiVersion: v1\nkind: Config\nclusters:\n- name: original-cluster\n  cluster:\n    server: http://127.0.0.1:{kube.server_port}\nusers:\n- name: original-user\n  user:\n    tokenFile: token\ncontexts:\n- name: local\n  context: {{cluster: original-cluster, user: original-user, namespace: ops}}\n- name: second\n  context: {{cluster: original-cluster, user: original-user, namespace: review}}\n"
        config_path=config_dir/'config';config_path.write_text('current-context: local\n'+source)
        port,native=free_port(),free_port();driver_process=launch([os.environ.get('OPSPIER_TAURI_DRIVER','tauri-driver'),'--port',str(port),'--native-port',str(native),'--native-host','127.0.0.1','--native-driver',os.environ.get('OPSPIER_WEBKIT_DRIVER','WebKitWebDriver')],'driver.log')
        driver=StableDriver(port)
        for _ in range(100):
            try:driver.call('GET','/status');break
            except (OSError,RuntimeError):time.sleep(.1)
        start_app('A-save');snapshot('01-home.png');check('categorized-sidebar-and-native-version')
        phase='sidebar-collapse'
        width=driver.js("return document.querySelector('.workspace-area').getBoundingClientRect().width")
        driver.button('收起连接')
        driver.wait("return !document.querySelector('.connection-sidebar').getClientRects().length && document.querySelector('.workspace-area').getBoundingClientRect().width>arguments[0]+200",width)
        driver.button('展开连接')
        driver.wait("return !!document.querySelector('.connection-sidebar').getClientRects().length")
        check('connection-sidebar-collapse-and-expand-gives-space-to-workspace')
        phase='unsaved-draft-tests'
        stop_keyring()
        def test_result(success=True):
            return driver.wait("return [...document.querySelectorAll('[data-testid=connection-test-result]')].find(e=>e.getClientRects().length&&e.textContent.includes(arguments[0]))",'测试连接成功' if success else '失败')
        def assert_unsaved():
            assert driver.ipc('profiles_list')['profiles']==[]
            assert driver.js("return document.querySelectorAll('.workspace-tabs [role=tab]').length===0")
            assert not list((private/'config').rglob('profiles-v1.json'))
            assert not list((private/'config').rglob('host-pins.json'))
        # SSH proof includes actual authentication with no channel, PTY or shell and no saved pin.
        new_profile('SSH')
        for field,value in {'host':'localhost','port':ssh_info['port'],'username':'fixture-user','password':'wrong-test-only'}.items():driver.fill('.ant-modal #'+field,value)
        driver.button('测试连接');driver.text(ssh_info['fingerprint']);driver.button('已核验，继续测试');test_result(False)
        driver.wait("return ![...document.querySelectorAll('.ant-modal')].some(e=>e.getClientRects().length&&e.querySelector('.ant-modal-title')?.textContent==='核验测试主机指纹')")
        driver.fill('.ant-modal #password',ssh_password)
        driver.wait("return ![...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length)")
        driver.js(r"const b=[...document.querySelectorAll('.ant-modal button')].find(e=>e.textContent.replace(/\s/g,'')==='测试连接');b.click();b.click();")
        driver.text(ssh_info['fingerprint']);driver.button('已核验，继续测试');test_result()
        events=[json.loads(line) for line in (private/'ssh-trace.jsonl').read_text().splitlines()]
        assert sum(e['event']=='authentication' and e.get('accepted') for e in events)==1
        assert not any(e['event'] in ['channel','pty','shell'] for e in events)
        assert driver.js("return document.querySelector('.ant-modal #name').value===''")
        assert_unsaved();driver.fill('.ant-modal #host','127.0.0.1');driver.wait("return ![...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length)");driver.button('取消');saved_modal_closed()
        check('unnamed-SSH-draft-real-auth-fingerprint-no-channel-shell-pin-workspace-and-duplicate-guard')
        for kind,port_default in [('MySQL',3306),('Apache Doris',9030)]:
            new_profile(kind);assert driver.js("return document.querySelector('.ant-modal #port').value") == str(port_default)
            for field,value in {'host':'localhost','port':db.server_address[1],'username':'fixture-user','password':'wrong-test-only'}.items():driver.fill('.ant-modal #'+field,value)
            before_auth=db.auth_success+db.auth_rejected
            assert not driver.js("return !!document.querySelector('.ant-modal #tls')")
            driver.button('测试连接');test_result(False)
            driver.fill('.ant-modal #password',password);start_trace=len(db.trace);driver.button('测试连接');test_result()
            queries=db.trace[start_trace:];assert 'SELECT 1' in queries
            if kind=='Apache Doris':assert not any(q=='START TRANSACTION READ ONLY' for q in queries)
            assert driver.js("return document.querySelector('.ant-modal #name').value===''");assert_unsaved();snapshot('09-draft-'+('doris' if kind=='Apache Doris' else 'mysql')+'-test.png');driver.button('取消');saved_modal_closed()
        check('unnamed-MySQL-Doris-drafts-verify-password-read-only-probe-default-ports-and-no-save')
        tls_db=start_tls_mysql(private/'tls-fixture',password)
        for kind,policy in [('doris','verifyIdentity'),('mysql','auto')]:
            driver.ipc('db_test_connection',{'config':{'name':'','host':'127.0.0.1','port':tls_db.server_address[1],'username':'fixture-user','password':password,'database':'','kind':kind,'tls':policy}})
        assert tls_db.tls_connections==2 and tls_db.auth_success==2
        check('MySQL-and-Doris-automatically-upgrade-to-real-TLS-without-user-policy-input')
        phase='TLS-peer-slow-close'
        tls_linger=start_tls_mysql(private/'tls-linger-fixture',password,shutdown_delay_seconds=3)
        started=time.monotonic()
        driver.ipc('db_test_connection',{'config':{'name':'','host':'127.0.0.1','port':tls_linger.server_address[1],'username':'fixture-user','password':password,'database':'','kind':'mysql','tls':'auto'}})
        elapsed=time.monotonic()-started;report['measurements']['TLS_peer_slow_close_test_ms']=round(elapsed*1000)
        assert elapsed<1.5,'completed read-only query must not wait for an idle TLS peer to close'
        check('completed-MySQL-probe-does-not-wait-for-a-slow-TLS-peer-close')
        phase='unsaved-draft-tests'
        new_profile('Kubernetes');driver.fill('input[aria-label=kubeconfig文件路径]',str(config_path));driver.button('测试连接');test_result();assert_unsaved()
        driver.fill('input[aria-label=kubeconfig文件路径]',str(config_path)+'-missing');driver.wait("return ![...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length)")
        driver.fill('input[aria-label=kubeconfig文件路径]',str(config_path));driver.button('解析配置');driver.text('2 个 Context');driver.select('.ant-modal #context','second');driver.fill('.ant-modal #namespace','review');driver.button('测试连接');test_result();assert_unsaved()
        driver.button('解析配置');driver.wait("return ![...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length)")
        driver.wait("return document.querySelector('.ant-modal #context').closest('.ant-select').textContent.includes('local')&&document.querySelector('.ant-modal #namespace').value==='ops'")
        driver.button('测试连接');test_result();driver.button('取消');saved_modal_closed()
        check('Kubernetes-reparse-changes-context-namespace-and-clears-old-test-success')
        # Large context lists stay a compact count and keep credentials inside input memory only.
        new_profile('Kubernetes');driver.select('.ant-modal .ant-select','粘贴配置文本')
        many='current-context: local\n'+source.replace('tokenFile: token','token: '+token)+''.join(f'- name: ctx-{n}\n  context: {{cluster: original-cluster, user: original-user, namespace: ops}}\n' for n in range(2,200))
        driver.fill('textarea[aria-label=kubeconfig配置文本]',many);driver.button('解析配置');driver.text('200 个 Context')
        assert driver.js("return document.querySelectorAll('.ant-modal .ant-tag').length<=2")
        driver.js("document.querySelector('textarea[aria-label=kubeconfig配置文本]').style.webkitTextSecurity='disc'")
        assert driver.js("return getComputedStyle(document.querySelector('textarea[aria-label=kubeconfig配置文本]')).webkitTextSecurity==='disc'")
        snapshot('10-compact-kube-import.png')
        driver.js("document.querySelector('textarea[aria-label=kubeconfig配置文本]').style.webkitTextSecurity=''")
        driver.button('测试连接');test_result();assert_unsaved();driver.button('取消');saved_modal_closed()
        assert not secret_bus_owned();daemon=start_keyring();db.trace.clear();kube.trace.clear()
        check('unnamed-Kubernetes-draft-safe-GET-no-profile-secret-service-or-runtime-and-compact-200-context-summary')
        phase='ssh-save-open'
        fill_connection('SSH','NFS服务',ssh_password);driver.button('保存连接');saved_modal_closed();profile_row('NFS服务')
        open_profile('NFS服务');driver.text(ssh_info['fingerprint']);driver.button('已核验并信任');driver.wait("return document.querySelector('[data-testid=ssh-status]')?.textContent==='已连接'")
        for _ in range(3):profile_action('NFS服务','新开');time.sleep(.25)
        driver.wait("return document.querySelectorAll('[data-testid=ssh-pane]').length===4");driver.select('.terminal-toolbar .ant-select','四分屏')
        driver.wait("return [...document.querySelectorAll('[data-testid=ssh-pane]')].filter(e=>e.getClientRects().length).length===4")
        assert driver.js("return [...document.querySelectorAll('[data-testid=ssh-endpoint]')].every(e=>e.textContent===arguments[0])",f"fixture-user@localhost:{ssh_info['port']}")
        assert driver.js("return [...document.querySelectorAll('[data-testid=ssh-pane]')].every(e=>!e.textContent.includes('NFS服务'))")
        pane_connections=[]
        def ssh_events():return [json.loads(line) for line in (private/'ssh-trace.jsonl').read_text().splitlines()]
        for index in range(4):
            terminal=driver.js("return document.querySelectorAll('[data-testid=ssh-pane]')[arguments[0]].querySelector('.xterm-helper-textarea')",index)
            marker=f'pane-check-{index+1}'
            driver.cmd('POST',f'/element/{terminal[ELEMENT]}/value',{'text':marker+'\nsize\n'})
            driver.wait("return document.querySelectorAll('[data-testid=ssh-pane]')[arguments[0]].querySelector('.xterm-rows').textContent.includes(arguments[1])",index,marker)
            digest=hashlib.sha256(marker.encode()).hexdigest();deadline=time.monotonic()+8
            while not any(e['event']=='echo' and e.get('digest')==digest for e in ssh_events()):
                if time.monotonic()>deadline:raise AssertionError('pane input not received by SSH fixture')
                time.sleep(.1)
            pane_connections.append(next(e['connection'] for e in ssh_events() if e['event']=='echo' and e.get('digest')==digest))
        assert len(set(pane_connections))==4
        before={cid:next(e for e in reversed(ssh_events()) if e['event']=='resize' and e.get('connection')==cid) for cid in pane_connections}
        trace_start=len(ssh_events());driver.cmd('POST','/window/rect',{'width':1250,'height':850});deadline=time.monotonic()+8
        while True:
            resized={e['connection']:e for e in ssh_events()[trace_start:] if e['event']=='resize' and e.get('connection') in pane_connections}
            if len(resized)==4 and all((resized[cid]['width'],resized[cid]['height'])!=(before[cid]['width'],before[cid]['height']) for cid in pane_connections):break
            if time.monotonic()>deadline:raise AssertionError('all four SSH PTYs must receive changed dimensions')
            time.sleep(.1)
        for index,cid in enumerate(pane_connections):
            terminal=driver.js("return document.querySelectorAll('[data-testid=ssh-pane]')[arguments[0]].querySelector('.xterm-helper-textarea')",index)
            driver.cmd('POST',f'/element/{terminal[ELEMENT]}/value',{'text':'size\n'})
            driver.wait("return document.querySelectorAll('[data-testid=ssh-pane]')[arguments[0]].querySelector('.xterm-rows').textContent.includes(arguments[1])",index,f"SIZE {resized[cid]['width']} {resized[cid]['height']}")
        report['measurements']['fourPaneResize']=[{'pane':index+1,'before':[before[cid]['width'],before[cid]['height']],'after':[resized[cid]['width'],resized[cid]['height']]} for index,cid in enumerate(pane_connections)]
        snapshot('11-four-pane-input-resize.png');driver.cmd('POST','/window/rect',{'width':1460,'height':1060});time.sleep(.4)
        check('four-SSH-panes-each-keyboard-echo-and-server-PTY-resize-with-returned-size')
        phase='sidebar-live-SSH'
        before_fold={cid:next(e for e in reversed(ssh_events()) if e['event']=='resize' and e.get('connection')==cid) for cid in pane_connections}
        auth_before=sum(e['event']=='authentication' and e.get('accepted',False) for e in ssh_events())
        driver.js("window.__sidebarSshNodes=[...document.querySelectorAll('[data-testid=ssh-pane]')]")
        width=driver.js("return document.querySelector('.workspace-area').getBoundingClientRect().width")
        offset=len(ssh_events());driver.button('收起连接')
        driver.wait("return !document.querySelector('.connection-sidebar').getClientRects().length&&document.querySelector('.workspace-area').getBoundingClientRect().width>arguments[0]+200",width)
        deadline=time.monotonic()+8
        while True:
            expanded={e['connection']:e for e in ssh_events()[offset:] if e['event']=='resize' and e.get('connection') in pane_connections}
            if len(expanded)==4 and all(expanded[cid]['width']>before_fold[cid]['width'] for cid in pane_connections):break
            if time.monotonic()>deadline:raise AssertionError('sidebar collapse must resize every existing SSH PTY')
            time.sleep(.1)
        assert driver.js("return window.__sidebarSshNodes.every((e,i)=>e.isConnected&&e===document.querySelectorAll('[data-testid=ssh-pane]')[i])")
        assert sum(e['event']=='authentication' and e.get('accepted',False) for e in ssh_events())==auth_before
        driver.button('展开连接');driver.wait("return !!document.querySelector('.connection-sidebar').getClientRects().length")
        check('sidebar-collapse-preserves-live-SSH-panes-and-resizes-existing-PTYs')
        snapshot('02-four-ssh-workspaces.png');profile_action('NFS服务','编辑');driver.fill('.ant-modal #name','SSH已保存');assert driver.js("return document.querySelector('.ant-modal input[type=password]')===null");driver.button('保存连接');saved_modal_closed()
        open_profile('SSH已保存',False);assert driver.js("return document.querySelectorAll('[data-testid=ssh-pane]').length") == 4
        check('SSH-saved-open-fingerprint-four-panes-and-rename-keeps-running-snapshot')
        phase='locked-credential-save'
        fill_connection('MySQL','MySQL已保存',password)
        locked=subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.secrets','--object-path','/org/freedesktop/secrets','--method','org.freedesktop.Secret.Service.Lock',"['/org/freedesktop/secrets/collection/login']"],capture_output=True,timeout=5);assert locked.returncode==0
        driver.button('保存连接')
        # A locked Secret Service may legitimately prompt. Decline only the prompter
        # owned by this private bus; no existing desktop or other app is inspected.
        deadline=time.monotonic()+25
        while not driver.js("return [...document.querySelectorAll('.ant-modal .ant-alert-error')].some(e=>e.getClientRects().length)"):
            if time.monotonic()>deadline:raise AssertionError('cancelled private unlock did not finish')
            reply=subprocess.run(['gdbus','call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.GetConnectionUnixProcessID','org.gnome.keyring.SystemPrompter'],capture_output=True,timeout=3)
            match=re.search(rb'uint32 (\d+)',reply.stdout)
            if match:
                result=subprocess.run([XDOTOOL,'search','--all','--onlyvisible','--pid',match.group(1).decode(),'--name','.*'],capture_output=True,text=True)
                for window in result.stdout.split():
                    subprocess.run([XDOTOOL,'windowfocus',window,'key','--window',window,'Escape'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            time.sleep(.2)
        # The prompter can destroy its window between injected keydown and keyup.
        # Explicitly release Escape on the private X server to prevent autorepeat cancelling later dialogs.
        subprocess.run([XDOTOOL,'keyup','Escape'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        assert driver.js("return document.querySelector('.ant-modal #password').value===arguments[0]",password)
        assert not any(p['name']=='MySQL已保存' for p in driver.ipc('profiles_list')['profiles'])
        snapshot('03-locked-service-retry.png');stop_keyring();daemon=start_keyring();driver.button('保存连接');saved_modal_closed();assert not db.trace
        check('locked-real-credential-service-rejects-save-keeps-input-and-unlocked-retry-succeeds-without-network')
        phase='mysql-query'
        open_profile('MySQL已保存');query_db()
        driver.click(driver.element('.workspace:not([hidden]) .ant-tree-switcher'));driver.text('items');driver.click(driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-tree-title')].find(e=>e.textContent==='items')"));driver.text('表结构');driver.text('varchar(255)');driver.button('索引');driver.text('PRIMARY')
        driver.click(driver.element('.workspace:not([hidden]) .ant-pagination-next button'));driver.text('row-050')
        driver.cmd('POST','/execute/async',{'script':"const done=arguments[arguments.length-1];document.querySelector('.workspace:not([hidden]) .ant-table-tbody-virtual-holder').dispatchEvent(new WheelEvent('wheel',{deltaY:400,bubbles:true,cancelable:true}));requestAnimationFrame(()=>requestAnimationFrame(()=>done(true)))",'args':[]})
        snapshot('04-mysql-results.png');driver.button('导出缓存 CSV')
        deadline=time.monotonic()+8
        while not list(private.rglob('*.csv')):
            if time.monotonic()>deadline:raise AssertionError('CSV download missing')
            time.sleep(.1)
        downloaded=list(private.rglob('*.csv'))[0];rows=list(csv.reader(io.StringIO(downloaded.read_text(encoding='utf-8-sig'))));assert len(rows)==251 and rows[1]==["'=formula"] and rows[2]==['中文😀'];shutil.copyfile(downloaded,output/'query-export.csv')
        driver.fill('.workspace:not([hidden]) textarea','SELECT 999');driver.button('执行只读查询');driver.button('取消');driver.text('cancelled')
        check('MySQL-verified-password-catalog-inspection-query-paging-CSV-and-early-cancel')
        phase='doris-save'
        fill_connection('Apache Doris','Doris已保存',password);driver.button('保存连接');saved_modal_closed();open_profile('Doris已保存');query_db();check('Doris-explicit-driver-and-verified-password-query')
        phase='database-edit-tests'
        for name in ['MySQL已保存','Doris已保存']:
            profile_action(name,'编辑');before=driver.ipc('profiles_list');tabs=driver.js("return document.querySelectorAll('.workspace-tabs [role=tab]').length")
            assert driver.js("return document.querySelector('.ant-modal input[type=password]')===null")
            driver.fill('.ant-modal #name','');driver.button('测试连接');test_result()
            assert driver.ipc('profiles_list')==before;assert driver.js("return document.querySelectorAll('.workspace-tabs [role=tab]').length")==tabs
            before_auth=db.auth_success+db.auth_rejected;driver.fill('.ant-modal #host','127.0.0.1');driver.button('测试连接');driver.text('认证目标已变化');assert db.auth_success+db.auth_rejected==before_auth
            driver.select('.ant-modal [aria-label=保存的凭据]','替换凭据');driver.fill('.ant-modal #password','wrong-only');driver.button('测试连接');driver.text('数据库认证失败')
            driver.fill('.ant-modal #password',password);driver.button('测试连接');test_result();assert driver.ipc('profiles_list')==before
            driver.fill('.ant-modal #host','localhost');db.forced_error=1049;driver.button('测试连接');driver.text('目标数据库不存在');assert not driver.js("return document.querySelector('[data-testid=connection-test-result]').textContent.includes(arguments[0])",password)
            db.forced_error=0;snapshot('12-database-edit-test.png');driver.button('取消');saved_modal_closed()
        check('MySQL-Doris-edit-Keep-replace-test-target-binding-safe-errors-and-no-save-or-runtime')
        phase='file-kube'
        import_kube('文件集群','file',config_path,skip_inspect=True)
        assert next(p for p in driver.ipc('profiles_list')['profiles'] if p['name']=='文件集群')['settings']['context']=='local'
        config_path.write_text(source);check('native-file-picker-relative-token-reference-import-and-default-without-preview')
        phase='drag-kube'
        import_kube('拖入集群','drag',config_path);check('native-GTK-uri-file-drag-through-Tauri-path-listener')
        phase='paste-kube'
        inline='current-context: local\n'+source.replace('tokenFile: token','token: '+token);import_kube('粘贴集群','text',text=inline);shutil.rmtree(config_dir);check('paste-static-config-import-and-source-files-removed-after-snapshot')
        phase='kube-RBAC-fallback'
        kube.deny_crd=True;kube.deny_namespaces=True;open_profile('粘贴集群');driver.text('CRD发现失败');driver.text('命名空间列表读取失败');driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','review');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ review')")
        driver.click(driver.element('[aria-label="关闭工作区 粘贴集群"]'));driver.wait("return !document.querySelector('[aria-label=\"关闭工作区 粘贴集群\"]')");kube.deny_crd=False;kube.deny_namespaces=False
        check('Kubernetes-CRD-namespace-discovery-403-keeps-builtins-and-manual-enter-switch-usable')
        phase='kube-browser-features'
        open_profile('文件集群');driver.wait("return document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-status-tag]').length>=3")
        driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-status-tag]')].some(e=>e.textContent==='Running')")
        colors=driver.js("return Object.fromEntries([...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-status-tag]')].map(e=>[e.textContent,getComputedStyle(e).color]))")
        assert len({colors['Running'],colors['Pending'],colors['Failed']})==3
        assert driver.js("return [...document.querySelectorAll('.workspace:not([hidden]) .workspace-toolbar button')].every(e=>!['切换','取消请求','关闭连接'].includes(e.textContent))")
        driver.select('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','review')
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ review')&&[...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-namespace-tag]')].every(e=>e.textContent==='review')")
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','ops');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'})
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ ops')")
        driver.button('下一页');driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第2页')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='api-2')")
        kube.expire_page_once=True;driver.button('刷新');driver.text('服务器分页已过期，已回到首页重新加载。');driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第1页')&&[...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        check('Kubernetes-expired-server-cursor-returns-to-first-page-with-an-explicit-notice')
        kube.many_search_rows=True;driver.fill('.workspace:not([hidden]) input[aria-label=搜索资源]','row-')
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('搜索')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-1')")
        driver.button('下一页');driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-101')");driver.button('下一页');driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第3页')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-201')")
        kube.denied_delay=.9;driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','denied');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'})
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-query-status]').textContent.includes('正在加载')")
        assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第3页')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-201')")
        driver.text('403');assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第3页')&&document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ ops')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-201')");snapshot('15-search-page3-failed-scope-preserved.png')
        kube.denied_delay=0;kube.list_delay=.4;driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','review');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'})
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-query-status]').textContent.includes('正在加载')");assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第3页')")
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ review')&&document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('第1页')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-1')")
        check('review-I1-local-search-third-page-and-scope-preserved-pending-failure-then-atomic-success')
        kube.list_delay=0;kube.header_error=403;driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','headers-error');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.text('403')
        assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ review')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='row-1')")
        deadline=time.monotonic()+4
        while not getattr(kube,'header_error_closed',0):
            if time.monotonic()>deadline:raise AssertionError('known HTTP failure did not close its hanging body')
            time.sleep(.05)
        check('review-I2-known-403-headers-drop-hanging-body-and-preserve-displayed-search')
        kube.header_error=0;kube.many_search_rows=False;driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','ops');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.fill('.workspace:not([hidden]) input[aria-label=搜索资源]','');driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        start_trace=len(kube.trace);driver.fill('.workspace:not([hidden]) input[aria-label=搜索资源]','late-only')
        driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='late-only')")
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('搜索')")
        queries=[q for q in kube.trace[start_trace:] if '/pods?' in q];assert any('continue=page3' in q for q in queries);assert all('/namespaces/ops/pods?' in q for q in queries)
        driver.fill('.workspace:not([hidden]) input[aria-label=搜索资源]','cross-target');driver.wait("return !document.querySelector('.workspace:not([hidden]) [data-testid=kube-query-status]').textContent.includes('正在加载')")
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('cross-target')")
        assert not driver.js("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='cross-target')")
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','')
        driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('全部命名空间')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='cross-target')")
        driver.fill('.workspace:not([hidden]) input[aria-label=搜索资源]','')
        driver.wait("return new Set([...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-namespace-tag]')].map(e=>e.textContent)).size===2")
        namespace_colors=driver.js("return Object.fromEntries([...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-namespace-tag]')].map(e=>[e.textContent,getComputedStyle(e).color]))")
        assert namespace_colors['ops']!=namespace_colors['review'];snapshot('13-kube-global-status-namespace-tags.png')
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','denied');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.text('403')
        assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('全部命名空间')&&[...document.querySelectorAll('.workspace:not([hidden]) .ant-table-tbody button')].filter(e=>e.textContent==='api').every(e=>e.disabled)")
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','ops');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ ops')&&[...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        kube.deny_all=True;driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','');driver.text('403')
        assert driver.js("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ ops')&&document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-status-tag]').length>=3")
        kube.deny_all=False;driver.button('刷新');driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('全部命名空间')&&!document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('只读快照')")
        driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','ops');field=driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]');driver.cmd('POST',f'/element/{field[ELEMENT]}/value',{'text':'\ue007'});driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-data-scope]').textContent.includes('/ ops')&&[...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        # Timer-driven refresh must retain rows and never cover them with a loading mask.
        while getattr(kube,'active_lists',0):time.sleep(.1)
        kube.list_delay=.8;kube.peak_lists=0;start_lists=sum('/pods?' in q for q in kube.trace)
        driver.js("window.__opsRefreshSamples=[];window.__opsRefreshTimer=setInterval(()=>{const w=document.querySelector('.workspace:not([hidden])');window.__opsRefreshSamples.push({rows:w.querySelectorAll('[data-testid=kube-status-tag]').length,mask:!!w.querySelector('.ant-spin-spinning'),pending:w.querySelector('[data-testid=kube-query-status]').textContent.includes('正在加载')});},40)")
        deadline=time.monotonic()+12
        while sum('/pods?' in q for q in kube.trace)<=start_lists:
            if time.monotonic()>deadline:raise AssertionError('automatic refresh never requested a list')
            time.sleep(.1)
        driver.wait("return window.__opsRefreshSamples.some(x=>x.pending)");driver.wait("return !document.querySelector('.workspace:not([hidden]) [data-testid=kube-query-status]').textContent.includes('正在加载')")
        samples=driver.js("clearInterval(window.__opsRefreshTimer);return window.__opsRefreshSamples");assert samples and all(x['rows']>=3 and not x['mask'] for x in samples);assert kube.peak_lists==1;background_peak=kube.peak_lists;kube.list_delay=0
        kube.force_list_error=403;driver.button('刷新');driver.text('403');assert driver.js("return document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-status-tag]').length>=3");kube.force_list_error=0;driver.button('刷新')
        select_resource('Widget · a.example/v2 · 命名空间');driver.button('widget-sample');driver.wait("return document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]')?.value.includes('a.example/v2')")
        select_resource('Widget · b.example/v1 · 集群');driver.button('widget-sample');driver.wait("return document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]')?.value.includes('b.example/v1')");assert driver.js("return [...document.querySelectorAll('.workspace:not([hidden]) [data-testid=kube-namespace-tag]')].every(e=>e.textContent==='集群级')")
        snapshot('14-kube-dynamic-cluster-resource.png')
        select_resource('CRD 定义');driver.button('widgets.a.example');driver.wait("return document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]')?.value.includes('CustomResourceDefinition')")
        select_resource('Secrets（仅元数据）');driver.button('secret-sample');driver.wait("return document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]')?.value.includes('kind: Secret')")
        assert driver.js("const y=document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]').value;return !y.includes('fixture-sensitive')&&!/^data:/m.test(y)&&!y.includes('annotations:')")
        assert any('/secrets' in q['path'] and 'PartialObjectMetadata' in q['accept'] for q in kube.accepts)
        select_resource('Pods');driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='api'&&!e.disabled)")
        report['measurements']['kubeRefreshSamples']=len(samples);report['measurements']['kubeBackgroundPeakRequests']=background_peak
        check('Kubernetes-status-namespace-colors-select-clear-all-scoped-full-page-search-RBAC-snapshot-refresh-and-dynamic-CRD-instances')
        phase='kube-workspaces'
        open_profile('文件集群');driver.text('api');driver.button('api');driver.wait("return document.querySelector('.workspace:not([hidden]) textarea[aria-label=资源YAML只读详情]')?.value.includes('kind: Pod')");driver.button('容器日志');driver.button('读取日志');driver.text('line-20');snapshot('05-kube-logs.png')
        open_profile('拖入集群');driver.text('api');driver.fill('.workspace:not([hidden]) input[aria-label=手动输入命名空间]','review');driver.cmd('POST',f"/element/{driver.element('.workspace:not([hidden]) input[aria-label=手动输入命名空间]')[ELEMENT]}/value",{'text':'\ue007'});driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent==='review')")
        open_profile('文件集群');driver.text('line-20');assert driver.js("return document.querySelector('.workspace:not([hidden]) input[aria-label=手动输入命名空间]').value")=='ops'
        assert driver.js("return document.querySelector('.workspace:not([hidden]) .workspace-toolbar').getBoundingClientRect().height<100");snapshot('06-stable-categorized-workspaces.png');check('separate-Kubernetes-context-namespace-log-workspaces-preserve-SSH-and-database-sessions')
        phase='cleared-default-context'
        profile_action('粘贴集群','编辑')
        driver.js("const e=document.querySelector('.ant-modal #context').closest('.ant-select');const clear=e.querySelector('.ant-select-clear');if(!clear)throw Error('clear control missing');clear.dispatchEvent(new MouseEvent('mousedown',{bubbles:true,button:0}));clear.click();")
        driver.button('保存集群');saved_modal_closed()
        cleared=next(p for p in driver.ipc('profiles_list')['profiles'] if p['name']=='粘贴集群');assert cleared['settings']['context'] is None
        result=driver.cmd('POST','/execute/async',{'script':"const done=arguments[arguments.length-1];window.__TAURI_INTERNALS__.invoke('profiles_open_kube',arguments[0]).then(()=>done({rejected:false}),()=>done({rejected:true}));",'args':[{'id':cleared['id'],'revision':cleared['revision'],'context':None}]});assert result['rejected']
        before_tabs=driver.js("return document.querySelectorAll('.workspace-tabs [role=tab]').length");open_profile('粘贴集群');driver.text('选择要打开的 Context');assert driver.js("return document.querySelectorAll('.workspace-tabs [role=tab]').length")==before_tabs
        driver.button('Cancel');check('cleared-saved-default-stays-null-and-reopen-requires-explicit-context')
        phase='single-instance'
        before=app_pids();subprocess.run([str(application)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=8);assert app_pids()==before;check('second-native-launch-focuses-existing-process-without-duplicate-sessions')
        phase='app-restart'
        profile_action('Doris已保存','编辑');saved_profile=next(p for p in driver.ipc('profiles_list')['profiles'] if p['name']=='Doris已保存');db.auth_delay=1;before_auth=db.auth_success;driver.button('测试连接')
        deadline=time.monotonic()+8
        while db.auth_success<=before_auth:
            if time.monotonic()>deadline:raise AssertionError('edited Keep did not authenticate')
            time.sleep(.05)
        driver.ipc('profiles_save',{'draft':{'id':saved_profile['id'],'expectedRevision':saved_profile['revision'],'name':saved_profile['name'],'settings':saved_profile['settings']},'credential':{'action':'keep'}})
        driver.text('连接配置已修改或删除');db.auth_delay=0;driver.button('取消');saved_modal_closed();check('edited-Keep-test-rejects-profile-revision-changed-during-real-network-probe')
        legacy_raw=json.dumps([{'id':'legacy-bad','kind':'ssh','name':'x'*81,'host':'localhost','port':ssh_info['port'],'username':'fixture-user'},{'id':'legacy-good','kind':'ssh','name':'旧版待补凭据','host':'localhost','port':ssh_info['port'],'username':'fixture-user'}])
        driver.js("localStorage.setItem('aic-ops.connections.v1',arguments[0])",legacy_raw)
        profile_path=list((private/'config').rglob('profiles-v1.json'))[0];config_parent=profile_path.parent
        stop_app();config_parent.chmod(0o500);start_app('B-app-restart')
        driver.text('有 2 项旧连接未迁移');assert len(driver.ipc('profiles_list')['profiles'])==6
        assert driver.js("return localStorage.getItem('aic-ops.connections.v1')===arguments[0]",legacy_raw)
        config_parent.chmod(0o700);check('migration-IO-failure-and-invalid-old-name-do-not-hide-existing-saved-list')
        reopen_all('app-restart');snapshot('07-app-restart-reopen.png')
        phase='daemon-restart'
        stop_app();stop_keyring();daemon=start_keyring();start_app('C-daemon-restart');driver.text('有 1 项旧连接未迁移');profile_row('旧版待补凭据');assert driver.js("return localStorage.getItem('aic-ops.connections.v1')===arguments[0]",legacy_raw);reopen_all('credential-daemon-restart');snapshot('08-daemon-restart-reopen.png')
        driver.js("window.__sidebarWorkspaceNodes=[...document.querySelectorAll('.workspace')];window.__sidebarActiveTab=document.querySelector('[role=tab][aria-selected=true]')")
        driver.button('收起连接');driver.wait("return !document.querySelector('.connection-sidebar').getClientRects().length")
        assert driver.js("return window.__sidebarWorkspaceNodes.every((e,i)=>e.isConnected&&e===document.querySelectorAll('.workspace')[i])&&window.__sidebarActiveTab===document.querySelector('[role=tab][aria-selected=true]')")
        driver.button('展开连接');driver.wait("return !!document.querySelector('.connection-sidebar').getClientRects().length")
        check('sidebar-toggle-keeps-active-workspace-and-existing-query-pages')
        phase='metadata-security'
        listing=driver.ipc('profiles_list');assert len(listing['profiles'])==7
        assert sum(p['credentialSaved'] for p in listing['profiles'])==6
        serialized=json.dumps(listing).encode();needles=[password.encode(),ssh_password.encode(),token.encode(),login]
        for secret in needles:assert secret not in serialized
        paths=list((private/'config').rglob('profiles-v1.json'));assert len(paths)==1;assert paths[0].stat().st_mode&0o777==0o600
        metadata=paths[0].read_bytes();assert not any(secret in metadata for secret in needles)
        profile=next(p for p in listing['profiles'] if p['name']=='MySQL已保存')
        changed=dict(profile['settings'],host='127.0.0.1')
        result=driver.cmd('POST','/execute/async',{'script':"const done=arguments[arguments.length-1];window.__TAURI_INTERNALS__.invoke('profiles_save',arguments[0]).then(()=>done({rejected:false}),()=>done({rejected:true}));",'args':[{'draft':{'id':profile['id'],'expectedRevision':profile['revision'],'name':profile['name'],'settings':changed},'credential':{'action':'keep'}}]});assert result['rejected'];check('target-change-cannot-keep-old-credential-through-real-IPC','ipc')
        phase='minimal-database-save'
        for kind,driver_kind in [('Apache Doris','doris'),('MySQL','mysql')]:
            new_profile(kind)
            for field,value in {'host':'localhost','port':db.server_address[1],'username':'fixture-user','password':password}.items():driver.fill('.ant-modal #'+field,value)
            assert not driver.js("return !!document.querySelector('.ant-modal #tls')")
            driver.button('保存连接');saved_modal_closed()
            generated='localhost:'+str(db.server_address[1]);profile_row(generated)
            saved=next(p for p in driver.ipc('profiles_list')['profiles'] if p['name']==generated)
            assert saved['settings']['driver']==driver_kind and saved['settings']['database']=='' and saved['credentialSaved']
            if driver_kind=='mysql':assert saved['settings']['tls']=='auto'
            profile_action(generated,'删除');driver.button('删除连接');driver.wait("return ![...document.querySelectorAll('.profile-name')].some(e=>e.textContent===arguments[0])",generated)
            saved_modal_closed()
            check(driver_kind+'-saves-without-name-database-or-TLS-input-and-keeps-password')
        phase='delete'
        profile_action('拖入集群','删除');driver.button('删除连接');driver.wait("return ![...document.querySelectorAll('.profile-name')].some(e=>e.textContent==='拖入集群')")
        assert driver.js("return document.querySelectorAll('[role=tab][aria-selected]').length")>=6
        check('delete-saved-profile-does-not-close-running-workspace')
        stop_app();config_parent.chmod(0o500);start_app('D-migration-idempotence');driver.text('有 1 项旧连接未迁移');assert len(driver.ipc('profiles_list')['profiles'])==6;assert driver.js("return localStorage.getItem('aic-ops.connections.v1')===arguments[0]",legacy_raw);config_parent.chmod(0o700);stop_app();check('valid-legacy-migration-is-idempotent-on-next-restart-without-rewriting-old-values')
        # Original input files have been deleted. Scan all remaining ordinary profile/cache/log files,
        # including encrypted keyring files, for synthetic raw/base64 material.
        inspected=0;variants=needles+[base64.b64encode(secret) for secret in needles]
        for base in [private,output]:
            for path in base.rglob('*'):
                if not path.is_file():continue
                data=path.read_bytes();inspected+=1
                if any(secret in data for secret in variants):raise AssertionError('synthetic secret found in ordinary file')
        report['measurements'].update(inspectedFiles=inspected,mysqlAuthenticatedConnections=db.auth_success,kubeAuthenticatedRequests=kube.auth_success)
        assert db.auth_success>0 and kube.auth_success>0
        check('ordinary-config-cache-log-and-encrypted-keyring-files-contain-no-raw-or-base64-synthetic-secrets','ipc')
        report['status']='passed'
    except Exception as error:
        report['status']='failed';report['phase']=phase;report['failureType']=type(error).__name__;report['safeDriverCategory']=str(error) if re.fullmatch(r'WebDriver [a-z ]{1,80}',str(error)) else None
        # Deliberately no exception message or request args (either may contain pasted secrets).
        report['failureStack']=[{'file':Path(f.filename).name,'line':f.lineno,'function':f.name} for f in traceback.extract_tb(error.__traceback__)]
        if driver and driver.sid:
            try:
                debug=driver.js("return {errors:window.__opsPageErrors,newButton:[...document.querySelectorAll('.connection-sidebar button.ant-btn-primary')].map(e=>({text:e.textContent,disabled:e.disabled})),modals:[...document.querySelectorAll('.ant-modal')].map(e=>({visible:!!e.getClientRects().length,title:e.querySelector('.ant-modal-title')?.textContent,fields:[...e.querySelectorAll('input,textarea')].map(i=>i.id||i.getAttribute('aria-label'))}))}")
                for item in debug.get('errors',[]):
                    if any(secret in item.get('message','') for secret in [password,ssh_password,token,login.decode()]):item['message']='[redacted]'
                report['debug']=debug
                driver.js("for(const e of document.querySelectorAll('textarea[aria-label=kubeconfig配置文本]'))e.value='[redacted test input]';")
                driver.js("for(const e of document.querySelectorAll('.ant-typography,.ant-alert,.ant-message'))if(arguments[0].some(s=>s&&e.textContent.includes(s)))e.textContent='[redacted synthetic secret]';",[password,ssh_password,token,login.decode()])
                driver.screenshot(output/'failure.png')
            except Exception:pass
        print('FAIL '+phase+' ('+type(error).__name__+')',flush=True)
    finally:
        if 'config_parent' in locals():config_parent.chmod(0o700)
        if driver and driver.sid:
            try:driver.cmd('DELETE','')
            except Exception:pass
        for process in reversed(processes):
            if process.poll() is None:
                try:os.killpg(process.pid,signal.SIGTERM)
                except ProcessLookupError:pass
        for process in processes:
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
                process.wait()
        for service in [db,kube,tls_db,tls_linger]:
            if service:service.shutdown();service.server_close()
        for log in logs:log.close()
        report['measurements']['webdriverReadRetries']=driver.read_retries if driver else 0
        report['cleanupEvidence']={'launchedProcessesExited':all(p.poll() is not None for p in processes),'ownedApplicationsExited':all(not Path(f'/proc/{pid}').exists() for pid in owned),'display196SocketRemoved':not Path('/tmp/.X11-unix/X196').exists()}
        (output/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    return 0 if report['status']=='passed' else 1


def main():
    parser=argparse.ArgumentParser();parser.add_argument('--inside',action='store_true');parser.add_argument('--previous-application',type=Path);parser.add_argument('--application',type=Path,default=Path(os.environ.get('CARGO_TARGET_DIR',str(CACHE/'target')))/'release/opspier');parser.add_argument('--output',type=Path,default=CACHE/'desktop-evidence');args=parser.parse_args();args.output.mkdir(parents=True,exist_ok=True)
    if args.inside:return inside(args.output,args.application,args.previous_application)
    with tempfile.TemporaryDirectory(prefix='ops-ui-') as directory:
        root=Path(directory)
        for part in ['config','data','cache','run','Downloads']:(root/part).mkdir(mode=0o700)
        (root/'config/user-dirs.dirs').write_text(f'XDG_DOWNLOAD_DIR="{root}/Downloads"\n')
        env={key:os.environ[key] for key in ['PKG_CONFIG_SYSROOT_DIR','CARGO_TARGET_DIR','AIC_OPS_FIXTURE_PYTHON_PATH','OPSPIER_TEST_CACHE','OPSPIER_XDOTOOL','OPSPIER_XVFB','OPSPIER_TAURI_DRIVER','OPSPIER_WEBKIT_DRIVER'] if key in os.environ}
        env.update(PATH='/usr/local/bin:/usr/bin:/bin',HOME=directory,DISPLAY=':196',XDG_CONFIG_HOME=str(root/'config'),XDG_DATA_HOME=str(root/'data'),XDG_CACHE_HOME=str(root/'cache'),XDG_RUNTIME_DIR=str(root/'run'),LD_LIBRARY_PATH=os.environ.get('LD_LIBRARY_PATH',''),LANG='C.UTF-8',LC_ALL='C.UTF-8',TAURI_WEBVIEW_AUTOMATION='true',WEBKIT_DISABLE_COMPOSITING_MODE='1',GIO_USE_VFS='local',NO_PROXY='127.0.0.1,localhost')
        run=subprocess.run(['dbus-run-session','--','/usr/bin/python3',str(Path(__file__).resolve()),'--inside','--application',str(args.application),'--output',str(args.output)]+(['--previous-application',str(args.previous_application)] if args.previous_application else []),env=env,timeout=600)
        # Native chooser services must not leave a FUSE mount inside our disposable runtime.
        own_mount=root/'run/gvfs'
        if os.path.ismount(own_mount):
            subprocess.run(['fusermount3','-u',str(own_mount)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5,check=True)
        result_path=args.output/'report.json'
        if result_path.exists():
            result=json.loads(result_path.read_text());result['cleanupEvidence']['privateDirectoryRemoved']=False;result_path.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
    path=args.output/'report.json'
    if path.exists():
        report=json.loads(path.read_text());report['cleanupEvidence']['privateDirectoryRemoved']=not root.exists();path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    return run.returncode

if __name__=='__main__':raise SystemExit(main())
