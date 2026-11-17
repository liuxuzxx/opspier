"""Owned loopback Kubernetes HTTP/WebSocket peer for the real kubectl client.
Only synthetic data; stdin is checked in memory and never added to diagnostics.
"""
import base64,hashlib,hmac,json,socket,struct,threading,time,ssl,subprocess,tempfile
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from urllib.parse import urlsplit,parse_qs

class Api(ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,label,token):
        super().__init__(('127.0.0.1',0),Handler);self.label=label;self.token=token
        self.private=tempfile.TemporaryDirectory(prefix='opspier-kubectl-api-');directory=Path(self.private.name);directory.chmod(0o700);key=directory/'key.pem';cert=directory/'cert.pem'
        ca_key=directory/'ca.key';ca_cert=directory/'ca.pem';csr=directory/'server.csr';extensions=directory/'server.ext'
        quiet={'check':True,'stdout':subprocess.DEVNULL,'stderr':subprocess.DEVNULL}
        subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(ca_key),'-out',str(ca_cert),'-days','1','-subj','/CN=OpsPier Fixture CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign'],**quiet)
        subprocess.run(['openssl','req','-new','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(csr),'-subj','/CN=localhost'],**quiet)
        extensions.write_text('basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n')
        subprocess.run(['openssl','x509','-req','-in',str(csr),'-CA',str(ca_cert),'-CAkey',str(ca_key),'-CAcreateserial','-out',str(cert),'-days','1','-extfile',str(extensions)],**quiet)
        for path in directory.iterdir():path.chmod(0o600)
        self.ca=base64.b64encode(ca_cert.read_bytes()).decode();context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(cert,key);self.socket=context.wrap_socket(self.socket,server_side=True)
        self.requests=[];self.denied=0;self.log_closed=0;self.watch_closed=0;self.exec_open=0;self.exec_unicode=False;self.exec_interrupt=False;self.resizes=[]
    def start(self):threading.Thread(target=self.serve_forever,daemon=True).start();return self
    @property
    def endpoint(self):return 'https://127.0.0.1:'+str(self.server_port)
    def close(self):self.shutdown();self.server_close();self.token=None;self.private.cleanup()
    def config(self,namespace=''):
        return {'apiVersion':'v1','kind':'Config','current-context':'same-context','clusters':[{'name':'same-cluster','cluster':{'server':self.endpoint,'certificate-authority-data':self.ca}}],'users':[{'name':'same-user','user':{'token':self.token}}],'contexts':[{'name':'same-context','context':{'cluster':'same-cluster','user':'same-user',**({'namespace':namespace} if namespace else {})}}]}

class Handler(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def reply(self,value,status=200):
        body=json.dumps(value,ensure_ascii=False).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    def pod(self,namespace):
        return {'apiVersion':'v1','kind':'Pod','metadata':{'name':self.server.label,'namespace':namespace,'uid':'fixture-'+self.server.label,'resourceVersion':'1','creationTimestamp':'2026-11-13T01:00:00Z','labels':{'app':'fixture'}},'spec':{'nodeName':'loopback-node','containers':[{'name':'app','image':'fixture:test'}]},'status':{'phase':'Running','containerStatuses':[{'name':'app','ready':True,'restartCount':0,'image':'fixture:test','imageID':'fixture-image','state':{'running':{'startedAt':'2026-11-13T01:00:00Z'}}}]}}
    def do_GET(self):
        if not hmac.compare_digest(self.headers.get('Authorization',''),'Bearer '+self.server.token):self.server.denied+=1;self.reply({'kind':'Status','apiVersion':'v1','status':'Failure','reason':'Unauthorized','message':'fixture unauthorized','code':401},401);return
        parsed=urlsplit(self.path);path=parsed.path;query=parse_qs(parsed.query);self.server.requests.append(path)
        ns=path.split('/namespaces/',1)[1].split('/',1)[0] if '/namespaces/' in path else 'default'
        if ns=='denied':self.server.denied+=1;self.reply({'apiVersion':'v1','kind':'Status','status':'Failure','message':'fixture RBAC denied','reason':'Forbidden','code':403},403);return
        if path=='/version':self.reply({'major':'1','minor':'31','gitVersion':'v1.31.10'});return
        if path=='/api':self.reply({'apiVersion':'v1','kind':'APIVersions','versions':['v1'],'serverAddressByClientCIDRs':[]});return
        if path=='/apis':self.reply({'apiVersion':'v1','kind':'APIGroupList','groups':[]});return
        if path=='/api/v1':
            self.reply({'apiVersion':'v1','kind':'APIResourceList','groupVersion':'v1','resources':[{'name':name,'singularName':kind.lower(),'namespaced':name!='namespaces','kind':kind,'verbs':verbs}for name,kind,verbs in [('pods','Pod',['get','list','watch']),('pods/log','Pod',['get']),('pods/exec','Pod',['get','create']),('events','Event',['list']),('namespaces','Namespace',['get','list'])]]});return
        if path.endswith('/exec'):
            self.websocket();return
        if path.endswith('/log') or query.get('watch',[''])[0].lower() in ['1','true']:
            watching=not path.endswith('/log');self.send_response(200);self.send_header('Content-Type','application/json' if watching else 'text/plain');self.send_header('Connection','close');self.end_headers();self.close_connection=True
            try:
                for number in range(200):
                    body=(json.dumps({'type':'ADDED' if number==0 else 'MODIFIED','object':self.pod(ns)},ensure_ascii=False)+'\n') if watching else self.server.label+' 中文日志 '+str(number)+'\n'
                    self.wfile.write(body.encode());self.wfile.flush();time.sleep(.1)
            except (OSError,ConnectionResetError):
                if watching:self.server.watch_closed+=1
                else:self.server.log_closed+=1
            return
        if path.endswith('/events'):self.reply({'apiVersion':'v1','kind':'EventList','metadata':{'resourceVersion':'1'},'items':[]});return
        if path=='/api/v1/namespaces':self.reply({'apiVersion':'v1','kind':'NamespaceList','metadata':{'resourceVersion':'1'},'items':[{'apiVersion':'v1','kind':'Namespace','metadata':{'name':name}}for name in ['default','first','second','denied']]});return
        if path.endswith('/pods'):self.reply({'apiVersion':'v1','kind':'PodList','metadata':{'resourceVersion':'1'},'items':[self.pod(ns)]});return
        if '/pods/' in path:self.reply(self.pod(ns));return
        self.reply({'apiVersion':'v1','kind':'Status','status':'Failure','reason':'NotFound','code':404},404)
    def frame(self,payload,opcode=2):
        length=len(payload);prefix=bytes([0x80|opcode,length]) if length<126 else bytes([0x80|opcode,126])+struct.pack('!H',length)
        self.wfile.write(prefix+payload);self.wfile.flush()
    def exact(self,n):
        value=self.rfile.read(n)
        if len(value)!=n:raise EOFError()
        return value
    def websocket(self):
        protocols=[v.strip()for v in self.headers.get('Sec-WebSocket-Protocol','').split(',')]
        if self.headers.get('Upgrade','').lower()!='websocket' or 'v5.channel.k8s.io' not in protocols:self.reply({'error':'expected Kubernetes v5 WebSocket'},400);return
        accepted=base64.b64encode(hashlib.sha1((self.headers['Sec-WebSocket-Key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        self.send_response(101);self.send_header('Upgrade','websocket');self.send_header('Connection','Upgrade');self.send_header('Sec-WebSocket-Accept',accepted);self.send_header('Sec-WebSocket-Protocol','v5.channel.k8s.io');self.end_headers();self.server.exec_open+=1;self.close_connection=True;self.request.settimeout(10)
        try:
            self.frame(b'\x01remote-ready\r\n$ ');stdin_tail=b'';unicode_seen=False
            while True:
                first,second=self.exact(2);length=second&127
                if length==126:length=struct.unpack('!H',self.exact(2))[0]
                elif length==127:length=struct.unpack('!Q',self.exact(8))[0]
                if length>65536:raise ValueError('fixture WS bound')
                mask=self.exact(4) if second&128 else b'';payload=self.exact(length)
                if mask:payload=bytes(v^mask[i%4] for i,v in enumerate(payload))
                if first&15==8:return
                if first&15==9:self.frame(payload,10);continue
                if not payload:continue
                channel,data=payload[0],payload[1:]
                if channel==4:
                    size=json.loads(data);self.server.resizes.append((size.get('Width'),size.get('Height')));continue
                if channel==0:
                    stdin_tail=(stdin_tail+data)[-32:]
                    if not unicode_seen and '中文'.encode() in stdin_tail:unicode_seen=True;self.server.exec_unicode=True;self.frame(b'\x01'+ 'remote-中文\r\n$ '.encode())
                    if b'\x03' in data:
                        self.server.exec_interrupt=True;self.frame(b'\x01remote-interrupted\r\n');self.frame(b'\x03'+json.dumps({'apiVersion':'v1','kind':'Status','status':'Success'}).encode());self.frame(b'',8);return
        except (OSError,EOFError,ValueError):return
    def do_PATCH(self):
        if not hmac.compare_digest(self.headers.get('Authorization',''),'Bearer '+self.server.token):self.reply({'kind':'Status','apiVersion':'v1','reason':'Unauthorized','code':401},401);return
        path=urlsplit(self.path).path;self.server.requests.append(path);length=int(self.headers.get('Content-Length','0'))
        if length>65536:self.reply({},413);return
        data=json.loads(self.rfile.read(length));self.server.mutated=data.get('metadata',{}).get('labels',{}).get('fixture')=='changed';pod=self.pod('default');pod['metadata']['labels']['fixture']='changed';self.reply(pod)
