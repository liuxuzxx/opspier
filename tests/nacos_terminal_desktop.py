"""New 0.1.9 scenarios on the real app, IPC, OS keyring, Nacos and kubectl."""
import json,os,secrets,time
from pathlib import Path
from desktop_driver import ELEMENT
from fixtures.nacos_server import NacosFixture
from fixtures.kubectl_api import Api

def verify(driver,private,needles,check,snapshot,new_profile,profile_action,open_profile,modal_closed,start_app,stop_app):
    assert os.environ.get('DISPLAY')==':196'
    password=secrets.token_hex(32);needles.append(password.encode());nacos=NacosFixture(Path(os.environ['OPSPIER_NACOS_DISTRIBUTION']),Path(os.environ['OPSPIER_NACOS_BIND_GUARD']),password);peers=[]
    def enter(selector):element=driver.element(selector);driver.cmd('POST',f'/element/{element[ELEMENT]}/value',{'text':'\ue007'})
    def confirm():driver.button('确认');driver.wait("return ![...document.querySelectorAll('.ant-modal-confirm')].some(e=>e.getClientRects().length)")
    def close_detail():
        driver.click(driver.element('.nacos-detail-drawer .ant-drawer-close'));driver.wait("return !document.querySelector('.nacos-detail-drawer.ant-drawer-open')&&![...document.querySelectorAll('.nacos-detail-drawer .ant-drawer-body')].some(e=>e.getClientRects().length)")
    def terminal_text(marker):driver.wait("return document.querySelector('.workspace:not([hidden]) .kube-terminal .xterm-rows')?.textContent.includes(arguments[0])",marker,timeout=20)
    def type_terminal(value):element=driver.element('.workspace:not([hidden]) .kube-terminal .xterm-helper-textarea');driver.cmd('POST',f'/element/{element[ELEMENT]}/value',{'text':value})
    commands=[0]
    def command(value,marker):
        commands[0]+=1;nonce=str(commands[0]);type_terminal(value+"; printf 'ui-done-%s\\n' '"+nonce+"'\n");terminal_text('ui-done-'+nonce);terminal_text(marker)
    def selected_namespace(value):driver.fill('.workspace:not([hidden]) input[aria-label="手动输入命名空间"]',value);enter('.workspace:not([hidden]) input[aria-label="手动输入命名空间"]')
    def import_cluster(name,peer):
        config=peer.config('first');config['contexts'].append({'name':'second-context','context':{'cluster':'same-cluster','user':'same-user','namespace':'second'}})
        new_profile('Kubernetes');driver.fill('.ant-modal #name',name);driver.select('.ant-modal .ant-select','粘贴配置文本');driver.fill('textarea[aria-label=kubeconfig配置文本]',json.dumps(config));driver.button('解析配置');driver.text('2 个 Context');driver.select('.ant-modal #context','same-context');driver.button('保存集群');modal_closed();open_profile(name);driver.wait("return [...document.querySelectorAll('.workspace:not([hidden]) .ant-table-cell')].some(e=>e.textContent===arguments[0])",peer.label)
    try:
        nacos.start();nacos.call('POST','/v1/console/namespaces',{'customNamespaceId':'fixture-blue','namespaceName':'Fixture'},nacos.token)
        key={'tenant':'fixture-blue','group':'BLUE','dataId':'file.yaml'}
        nacos.call('POST','/v1/cs/configs',{**key,'content':'initial: value\r\n','type':'yaml','appName':'fixture-app'},nacos.token)
        for group,data_id in [('BLUE','other.yaml'),('RED','file-other.yaml')]:nacos.call('POST','/v1/cs/configs',{'tenant':'fixture-blue','group':group,'dataId':data_id,'content':'fixture: noise','type':'yaml'},nacos.token)
        nacos.call('POST','/v1/ns/service',{'namespaceId':'public','serviceName':'BLUE@@api','protectThreshold':'.2','selector':'{"type":"none"}'},nacos.token)
        nacos.call('POST','/v1/ns/instance',{'namespaceId':'public','serviceName':'BLUE@@api','ip':'127.0.0.1','port':'12345','clusterName':'ALT','ephemeral':'false','enabled':'true','weight':'1','metadata':'{"keep":"value"}'},nacos.token)
        start_app('H-nacos-terminal');new_profile('Nacos')
        for name,value in {'name':'配置测试','endpoint':nacos.endpoint,'namespace':'fixture-blue','username':'nacos','password':'wrong-owned-fixture'}.items():driver.fill('.ant-modal #'+name,value)
        before=len(driver.ipc('profiles_list')['profiles']);driver.button('测试连接');driver.wait("return [...document.querySelectorAll('[data-testid=connection-test-result]')].some(e=>e.getClientRects().length&&!e.textContent.includes('测试连接成功'))")
        driver.fill('.ant-modal #password',password);driver.button('测试连接');driver.text('测试连接成功');assert len(driver.ipc('profiles_list')['profiles'])==before
        driver.button('保存连接');modal_closed();open_profile('配置测试')
        driver.wait("return document.querySelectorAll('.nacos-list tbody tr[data-row-key]').length===3");snapshot('11-nacos-list.png')
        driver.fill('[aria-label="Nacos Group查询"]','BL');enter('[aria-label="Nacos Group查询"]');driver.wait("return document.querySelectorAll('.nacos-list tbody tr[data-row-key]').length===2")
        driver.fill('[aria-label="Nacos名称查询"]','file');enter('[aria-label="Nacos名称查询"]');driver.wait("return document.querySelectorAll('.nacos-list tbody tr[data-row-key]').length===1")
        check('Nacos-real-server-Group-and-Data-ID-substring-search-by-enter')
        driver.button('file.yaml');driver.wait("return document.querySelector('[aria-label=Nacos配置内容]')?.value.includes('initial: value')")
        check('Nacos-real-authentication-draft-test-no-save-OS-keyring-save-and-config-view')
        driver.fill('[aria-label=Nacos配置内容]','ui: 中文\n');driver.button('发布配置');driver.text('发布配置？');confirm();driver.text('服务端读回已核验。')
        assert json.loads(nacos.call('GET','/v1/cs/configs',{**key,'show':'all'},nacos.token))['content']=='ui: 中文\n'
        driver.button('配置历史');driver.button('对照');driver.wait("return !!document.querySelector('[aria-label=历史配置对照]')")
        history=json.loads(nacos.call('GET','/v1/cs/history',{**key,'search':'accurate','pageNo':1,'pageSize':20},nacos.token))
        original=next(row for row in history['pageItems']if json.loads(nacos.call('GET','/v1/cs/history',{**key,'nid':row['id']},nacos.token))['content']=='initial: value\r\n')
        row=driver.wait("return [...document.querySelectorAll('.ant-modal tr')].find(e=>e.querySelector('td')?.textContent===String(arguments[0]))",original['id']);driver.click(driver.js("return [...arguments[0].querySelectorAll('button')].find(e=>e.textContent.replace(/\\s/g,'')==='回滚')",row));confirm();modal_closed();driver.wait("return document.querySelector('[aria-label=Nacos配置内容]')?.value.includes('initial: value')")
        driver.button('删除配置');driver.button('取消');modal_closed();assert nacos.call('GET','/v1/cs/configs',key,nacos.token)==b'initial: value\r\n';driver.button('删除配置');confirm();driver.wait("return !document.querySelector('.nacos-detail-drawer.ant-drawer-open')&&![...document.querySelectorAll('.nacos-detail-drawer .ant-drawer-body')].some(e=>e.getClientRects().length)")
        driver.fill('[aria-label="Nacos Group查询"]','');driver.fill('[aria-label="Nacos名称查询"]','');enter('[aria-label="Nacos名称查询"]');driver.wait("return document.querySelectorAll('.nacos-list tbody tr[data-row-key]').length===2")
        driver.button('新建配置');driver.fill('[aria-label="配置Data ID"]','created.yaml');driver.fill('[aria-label=Nacos配置内容]','new: value\n');driver.button('发布配置');confirm();driver.text('服务端读回已核验。');check('Nacos-real-config-publish-history-compare-rollback-delete-confirm-and-create')
        close_detail();driver.fill('.nacos-panel input[role=combobox]','public');enter('.nacos-panel input[role=combobox]');driver.button('服务与实例');driver.button('api');driver.wait("return !!document.querySelector('[aria-label=服务保护阈值]')")
        driver.fill('[aria-label=服务保护阈值]','0.3');driver.fill('[aria-label=服务元数据]','{"ui":"yes"}');driver.button('更新服务属性');confirm();driver.text('服务端读回已核验。')
        driver.button('127.0.0.1:12345');driver.click(driver.element('[aria-label=实例启用]'));driver.fill('[aria-label=实例权重]','2.5');driver.fill('[aria-label=实例元数据]','{"keep":"value","ui":"yes"}');driver.button('更新实例');confirm();driver.wait("return [...document.querySelectorAll('.nacos-detail-drawer .ant-alert-success')].some(e=>e.textContent.includes('服务端读回已核验'))");driver.wait("return document.querySelector('[aria-label=实例启用]')?.getAttribute('aria-checked')==='false'")
        instances=json.loads(nacos.call('GET','/v1/ns/catalog/instances',{'namespaceId':'public','serviceName':'BLUE@@api','clusterName':'ALT','pageNo':1,'pageSize':20},nacos.token))['list'];assert len(instances)==1 and not instances[0]['enabled']and instances[0]['weight']==2.5
        check('Nacos-real-service-selector-preservation-and-disabled-instance-remains-visible')
        snapshot('12-nacos.png');close_detail();profile_action('配置测试','编辑');assert not driver.js("return !!document.querySelector('.nacos-dialog #password')");driver.button('测试连接');driver.text('测试连接成功');driver.button('取消');modal_closed()
        driver.button('配置');driver.button('新建配置');driver.fill('[aria-label="配置Data ID"]','unsaved.yaml');driver.click(driver.element('[aria-label="关闭工作区 配置测试"]'));driver.text('放弃未保存的修改？');driver.button('取消');modal_closed();assert driver.js("return !!document.querySelector('[aria-label=\"关闭工作区 配置测试\"]')")
        driver.click(driver.element('[aria-label="关闭工作区 配置测试"]'));confirm();check('Nacos-close-dirty-draft-cancel-retains-workspace-and-confirm-closes')
        stop_app();start_app('I-nacos-keyring-reopen');open_profile('配置测试');driver.button('created.yaml');check('Nacos-saved-credential-reopens-after-app-restart-without-secret-refill')
        close_detail();peers=[Api(label,secrets.token_hex(24)).start()for label in ['alpha','beta']];needles.extend(peer.token.encode()for peer in peers)
        import_cluster('终端甲',peers[0]);driver.select('.workspace:not([hidden]) [aria-label=Kubernetes视图]','kubectl 终端');driver.button('打开终端');terminal_text('kubectl> ')
        command("printf '\\u4e2d\\u6587\\n'; kubectl get pods -o name",'pod/alpha');terminal_text('中文');driver.js("window.__terminalNode=document.querySelector('.workspace:not([hidden]) .kube-terminal .xterm')")
        before=len(peers[0].requests);type_terminal('kubec\ue004 get pods -o name\n');deadline=time.monotonic()+8
        while len(peers[0].requests)<=before and time.monotonic()<deadline:time.sleep(.1)
        assert len(peers[0].requests)>before;before=len(peers[0].requests);type_terminal('\ue013\n');deadline=time.monotonic()+8
        while len(peers[0].requests)<=before and time.monotonic()<deadline:time.sleep(.1)
        assert len(peers[0].requests)>before
        driver.ipc('plugin:clipboard-manager|write_text',{'text':"printf '\\u7c98\\u8d34\\n'"});driver.button('粘贴');type_terminal('\n');terminal_text('粘贴')
        type_terminal('kubectl logs -f alpha\n');terminal_text('alpha 中文日志');driver.select('.workspace:not([hidden]) [aria-label=Kubernetes视图]','资源浏览');time.sleep(.3);driver.select('.workspace:not([hidden]) [aria-label=Kubernetes视图]','kubectl 终端');assert driver.js("return window.__terminalNode===document.querySelector('.workspace:not([hidden]) .kube-terminal .xterm')");type_terminal('\u0003');terminal_text('kubectl> ')
        selected_namespace('second');driver.wait("return document.querySelector('.workspace:not([hidden]) .kube-terminal')?.textContent.includes('same-context / second')");command('kubectl get pods -o name','pod/alpha');assert '/api/v1/namespaces/second/pods'in peers[0].requests
        selected_namespace('');command('kubectl get pods -o name','pod/alpha');assert '/api/v1/namespaces/default/pods'in peers[0].requests
        check('native-xterm-keyboard-Unicode-real-kubectl-mode-retention-and-namespace-default')
        type_terminal('kubectl exec -it alpha -- sh\n');terminal_text('remote-ready');type_terminal('中文\n');terminal_text('remote-中文');before=len(peers[0].resizes);driver.cmd('POST','/window/rect',{'width':1250,'height':850});deadline=time.monotonic()+5
        while len(peers[0].resizes)<=before and time.monotonic()<deadline:time.sleep(.1)
        assert len(peers[0].resizes)>before;type_terminal('\u0003');terminal_text('remote-interrupted');assert peers[0].exec_interrupt and peers[0].exec_unicode
        command('kubectl get pods -n denied','fixture RBAC denied');command('kubectl label pod alpha fixture=changed --overwrite','labeled');assert peers[0].mutated
        check('native-xterm-real-WebSocket-exec-input-resize-CtrlC-RBAC-and-write-command')
        command("sleep 60 & printf 'child-%s\\n' marker",'child-marker');driver.select('.workspace:not([hidden]) [aria-label=Context]','second-context');driver.wait("return document.querySelector('.workspace:not([hidden]) [data-testid=kube-terminal-status]')?.textContent==='未打开'")
        root=private/'cache/com.aic.ops/kubectl-terminals-v1';deadline=time.monotonic()+6
        while root.exists()and list(root.iterdir())and time.monotonic()<deadline:time.sleep(.1)
        assert not root.exists()or not list(root.iterdir());driver.button('打开终端');terminal_text('kubectl> ');command('kubectl get pods -o name','pod/alpha')
        import_cluster('终端乙',peers[1]);driver.select('.workspace:not([hidden]) [aria-label=Kubernetes视图]','kubectl 终端');driver.button('打开终端');terminal_text('kubectl> ');command('kubectl get pods -o name','pod/beta');assert not peers[1].exec_open and not getattr(peers[1],'mutated',False)
        snapshot('13-kubectl-terminal.png');check('native-Context-switch-reaps-old-processes-explicit-reopen-and-same-name-connection-isolation')
        stop_app()
        start_app('J-terminal-crash-recovery')
        assert not root.exists()or not list(root.iterdir())
        check('forced-driver-exit-recovers-abandoned-private-kubeconfigs-on-next-start')
        open_profile('终端甲');driver.select('.workspace:not([hidden]) [aria-label=Kubernetes视图]','kubectl 终端');driver.button('打开终端');terminal_text('kubectl> ');command('kubectl get pods -o name','pod/alpha')
        stop_app(graceful=True);deadline=time.monotonic()+6
        while root.exists()and list(root.iterdir())and time.monotonic()<deadline:time.sleep(.1)
        assert not root.exists()or not list(root.iterdir());assert not(private/'.bash_history').exists();check('native-window-close-cleans-PTY-processes-and-private-kubeconfigs-without-shell-history')
    finally:
        for peer in peers:peer.close()
        nacos.close()
