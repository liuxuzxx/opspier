"""Native MinIO desktop scenarios, invoked from the private full application suite."""
import hashlib,json,os,select,subprocess,time
from pathlib import Path
from desktop_driver import ELEMENT
from fixtures.minio_server import MinioFixture

def verify(driver,private,processes,needles,check,snapshot,new_profile,profile_action,open_profile,saved_modal_closed,start_app,stop_app,restart_keyring):
    assert os.environ.get('DISPLAY')==':196','GUI automation requires the private display'
    xdotool=os.environ.get('OPSPIER_XDOTOOL','xdotool')
    fixture=MinioFixture(os.environ.get('OPSPIER_MINIO_SERVER',''))
    processes.append(fixture.process);needles.extend([fixture.access.encode(),fixture.secret.encode()])
    inputs=private/'minio-inputs';inputs.mkdir()
    source=inputs/'report space.txt';source.write_bytes(b'file chooser upload\0'+bytes(range(256)))
    dragged=inputs/'报告.txt';dragged.write_bytes('通过原生拖入上传'.encode())
    big=inputs/'multipart.bin';big.write_bytes(bytes(range(256))*(9*1024*1024//256))
    def row(key):return driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-tbody tr')].find(e=>e.textContent.includes(arguments[0]))",key)
    def action(key,label):driver.click(driver.wait("return [...arguments[0].querySelectorAll('button')].find(e=>e.textContent.replace(/\s/g,'')===arguments[1]&&!e.disabled)",row(key),label))
    def field(selector,value):driver.fill(selector,value)
    def destination(prefix):field('.workspace:not([hidden]) [aria-label=对象前缀]',prefix);driver.button('打开前缀');driver.wait("return !document.querySelector('.workspace:not([hidden]) .ant-spin-spinning')");time.sleep(.1)
    def idle():driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) button')].some(e=>e.textContent==='上传文件'&&!e.disabled)",timeout=25)
    def native_choose(title,path):
        deadline=time.monotonic()+8;windows=[]
        while not windows:
            windows=subprocess.run([xdotool,'search','--all','--onlyvisible','--name',title],capture_output=True,text=True).stdout.split()
            if time.monotonic()>deadline:raise AssertionError('owned native file dialog unavailable')
            time.sleep(.1)
        subprocess.run([xdotool,'windowfocus','--sync',windows[-1]],check=True)
        subprocess.run([xdotool,'key','--clearmodifiers','alt+n' if title=='保存 MinIO 文件' else 'ctrl+l'],check=True)
        if title=='保存 MinIO 文件':subprocess.run([xdotool,'key','--clearmodifiers','ctrl+a'],check=True)
        subprocess.run([xdotool,'type','--clearmodifiers',str(path)],check=True)
        subprocess.run([xdotool,'key','Return'],check=True);time.sleep(.4)
        still_open=subprocess.run([xdotool,'search','--all','--onlyvisible','--name',title],capture_output=True,text=True).stdout.split()
        if still_open:subprocess.run([xdotool,'key','Return'],check=True)
    def drag(path):
        before_drop=driver.js('return window.__minioNativeDrops||0')
        helper=subprocess.Popen(['/usr/bin/python3',str(Path(__file__).parent/'fixtures/drag_source.py')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,start_new_session=True);processes.append(helper)
        helper.stdin.write(json.dumps({'path':str(path)}).encode()+b'\n');helper.stdin.flush();assert helper.stdout.readline().strip()==b'ready'
        windows=[];deadline=time.monotonic()+5
        while not windows:
            windows=subprocess.run([xdotool,'search','--all','--onlyvisible','--pid',str(helper.pid),'--name','OPS fixture file drag'],capture_output=True,text=True).stdout.split()
            if time.monotonic()>deadline:raise AssertionError('owned drag source unavailable')
            time.sleep(.1)
        raw=subprocess.check_output([xdotool,'getwindowgeometry','--shell',windows[-1]],text=True);geometry={k:int(v) for k,v in (line.split('=',1) for line in raw.splitlines())}
        sx=geometry['X']+geometry['WIDTH']//2;sy=geometry['Y']+geometry['HEIGHT']//2
        bounds=driver.js("const r=document.querySelector('.workspace:not([hidden])').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+100};")
        frame=driver.cmd('GET','/window/rect');tx=int(frame['x']+bounds['x']);ty=int(frame['y']+bounds['y'])
        subprocess.run([xdotool,'mousemove','--sync',str(sx),str(sy),'mousedown','1'],check=True);time.sleep(.2)
        for part in [.05,.15,.35,.6,.8,1]:
            subprocess.run([xdotool,'mousemove','--sync',str(int(sx+(tx-sx)*part)),str(int(sy+(ty-sy)*part))],check=True);time.sleep(.15)
        time.sleep(.4);subprocess.run([xdotool,'mouseup','1'],check=True);time.sleep(.3)
        events=[]
        while select.select([helper.stdout],[],[],.1)[0]:
            event=helper.stdout.readline().decode().strip()
            if event in ['begin','end','data','failed']:events.append(event)
            else:break
        driver.wait('return window.__minioNativeDrops>arguments[0]',before_drop)
        helper.terminate();helper.wait(timeout=3);assert 'begin' in events,'native GTK drag source must initiate the drag'
    def completed(direction='上传'):driver.wait("const t=document.querySelector('.workspace:not([hidden]) [data-testid=minio-transfer]')?.textContent||'';return t.includes('已完成')&&t.includes(arguments[0])",direction,timeout=30);idle()
    try:
        start_app('E-minio')
        capture=driver.js('window.__minioNativeDrops=0;return window.__TAURI_INTERNALS__.transformCallback(()=>window.__minioNativeDrops++)')
        driver.ipc('plugin:event|listen',{'event':'tauri://drag-drop','target':{'kind':'Webview','label':'main'},'handler':capture})
        new_profile('MinIO')
        for name,value in {'name':'对象测试','endpoint':fixture.endpoint,'bucket':'fixture-bucket','accessKey':fixture.access,'secretKey':'wrong-fixture-only'}.items():field('.ant-modal #'+name,value)
        before=len(driver.ipc('profiles_list')['profiles']);driver.button('测试连接');driver.wait("return [...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length&&!e.textContent.includes('测试连接成功'))")
        field('.ant-modal #secretKey',fixture.secret);driver.button('测试连接');driver.text('测试连接成功');assert len(driver.ipc('profiles_list')['profiles'])==before
        driver.button('保存连接');saved_modal_closed();open_profile('对象测试');row('folder/');row('page-000.txt');driver.button('下一页');row('page-204.txt');driver.button('上一页');row('folder/')
        check('MinIO-real-server-invalid-key-correct-key-test-save-list-buckets-and-paged-objects')
        # All category controls are independent and keep the same running workspace nodes.
        driver.js("window.__categoryNodes=[...document.querySelectorAll('.workspace')];window.__categoryTab=document.querySelector('[role=tab][aria-selected=true]')")
        for name in ['主机与终端','数据库','Kubernetes','对象存储']:
            button=driver.element('[aria-label="收起'+name+'"]');driver.click(button)
            assert driver.js("return arguments[0].getAttribute('aria-expanded')==='false'&&!document.getElementById(arguments[0].getAttribute('aria-controls')).getClientRects().length",button)
        assert driver.js("return window.__categoryNodes.every((e,i)=>e===document.querySelectorAll('.workspace')[i])&&window.__categoryTab===document.querySelector('[role=tab][aria-selected=true]')")
        for name in ['主机与终端','数据库','Kubernetes','对象存储']:driver.click(driver.element('[aria-label="展开'+name+'"]'))
        button=driver.element('[aria-label="收起数据库"]');driver.js('arguments[0].focus()',button)
        driver.cmd('POST','/actions',{'actions':[{'type':'key','id':'category-key','actions':[{'type':'keyDown','value':' '},{'type':'keyUp','value':' '}]}]})
        driver.wait("return document.querySelector('[aria-label=展开数据库]')?.getAttribute('aria-expanded')==='false'");driver.click(driver.element('[aria-label="展开数据库"]'))
        check('all-category-collapse-independent-accessible-keyboard-and-preserves-existing-workspaces')
        destination('uploads/')
        driver.button('上传文件');native_choose('选择上传文件',source);completed();row('report space.txt');assert fixture.request('GET','/fixture-bucket/uploads/report space.txt')==source.read_bytes()
        drag(dragged);completed();row('报告.txt');assert fixture.request('GET','/fixture-bucket/uploads/报告.txt')==dragged.read_bytes();check('MinIO-native-chooser-and-GTK-drag-upload-exact-bytes-including-Unicode')
        source.write_bytes(b'replacement bytes')
        driver.button('上传文件');native_choose('选择上传文件',source);driver.text('覆盖同名对象？');driver.button('取消上传');saved_modal_closed();idle();assert fixture.request('GET','/fixture-bucket/uploads/report space.txt')!=source.read_bytes()
        driver.button('上传文件');native_choose('选择上传文件',source);driver.text('覆盖同名对象？');driver.button('覆盖对象');saved_modal_closed();completed();assert fixture.request('GET','/fixture-bucket/uploads/report space.txt')==source.read_bytes();check('MinIO-overwrite-decline-preserves-object-and-confirm-replaces-it')
        target=private/'Downloads/minio-download.txt'
        action('report space.txt','下载');native_choose('保存 MinIO 文件',target);completed('下载')
        assert target.read_bytes()==source.read_bytes();check('MinIO-native-download-dialog-saves-exact-object-bytes')
        action('报告.txt','删除');driver.text('删除此对象？');driver.button('取消');saved_modal_closed();assert fixture.exists('uploads/报告.txt')
        action('报告.txt','删除');driver.button('删除对象');saved_modal_closed();driver.text('对象已删除');assert not fixture.exists('uploads/报告.txt');check('MinIO-delete-cancel-and-confirm-affect-only-the-selected-object')
        # A >8 MiB file must pass through multipart APIs on the real server.
        driver.button('上传文件');native_choose('选择上传文件',big);completed();assert hashlib.sha256(fixture.request('GET','/fixture-bucket/uploads/multipart.bin')).digest()==hashlib.sha256(big.read_bytes()).digest();check('MinIO-real-multipart-upload-has-identical-content')
        snapshot('09-minio-files.png')
        profile_action('对象测试','编辑');assert not driver.js("return !!document.querySelector('.minio-dialog #accessKey')||!!document.querySelector('.minio-dialog #secretKey')")
        driver.button('测试连接');driver.text('测试连接成功');field('.ant-modal #endpoint','http://localhost:1');driver.button('测试连接');driver.text('认证目标已变化');driver.button('取消');saved_modal_closed();check('MinIO-edited-Keep-tests-with-saved-keys-but-rejects-target-change')
        guarded=inputs/'guarded.txt';guarded.write_bytes(b'visible workspace only')
        new_profile('MySQL');drag(guarded);time.sleep(.4);assert not fixture.exists('uploads/guarded.txt');driver.button('取消');saved_modal_closed();check('MinIO-native-drop-event-during-connection-modal-does-not-upload')
        new_profile('MinIO')
        for name,value in {'name':'对象另一个','endpoint':fixture.endpoint,'bucket':'second-bucket','accessKey':fixture.access,'secretKey':fixture.secret}.items():field('.ant-modal #'+name,value)
        driver.button('保存连接');saved_modal_closed();open_profile('对象另一个');drag(guarded);completed()
        assert fixture.request('GET','/second-bucket/guarded.txt')==guarded.read_bytes()
        assert not fixture.exists('uploads/guarded.txt') and not fixture.exists('guarded.txt');check('MinIO-native-drop-only-uploads-to-the-active-workspace')
        listing=driver.ipc('profiles_list');public=json.dumps(listing).encode();assert fixture.access.encode() not in public and fixture.secret.encode() not in public
        stop_app();start_app('F-minio-app-restart');open_profile('对象测试');row('folder/');destination('uploads/');row('report space.txt');check('MinIO-reopens-after-application-restart-without-reentering-keys')
        stop_app();restart_keyring();start_app('G-minio-keyring-restart');open_profile('对象测试');row('folder/');destination('uploads/');row('multipart.bin');check('MinIO-reopens-after-credential-service-restart-without-reentering-keys')
        stop_app()
    finally:
        fixture.close()
