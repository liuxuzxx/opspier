"""Synthetic loopback services for real desktop verification; no real credentials."""
import json
import hashlib
import hmac
import socketserver
import struct
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit, parse_qs


def field(value):
    data = str(value).encode()
    if len(data) < 251:
        return bytes([len(data)]) + data
    return b"\xfc" + struct.pack("<H", len(data)) + data


class Mysql(socketserver.BaseRequestHandler):
    def packet(self, sequence, body):
        self.request.sendall(len(body).to_bytes(3, "little") + bytes([sequence % 256]) + body)

    def read(self):
        def exact(n):
            data = b""
            while len(data) < n:
                part = self.request.recv(n - len(data))
                if not part:
                    raise EOFError()
                data += part
            return data
        n = int.from_bytes(exact(4)[:3], "little")
        if n > 65536:
            raise ValueError("fixture request too large")
        return exact(n)

    def column(self, name):
        return b"".join(field(v) for v in ["def", "demo", "items", "items", name, name]) + bytes([12,33,0,255,0,0,0,253,0,0,0,0,0])

    def result(self, names, rows):
        eof = bytes([254,0,0,2,0])
        self.packet(1, bytes([len(names)]))
        seq = 2
        for name in names:
            self.packet(seq, self.column(name)); seq += 1
        self.packet(seq, eof); seq += 1
        for row in rows:
            self.packet(seq, b"".join(field(v) for v in row)); seq += 1
        self.packet(seq, eof)

    def handle(self):
        caps = 1 | 4 | 8 | 512 | 8192 | 32768 | (1 << 19)
        hello = b"\x0a8.0.36-desktop-fixture\0" + struct.pack("<I", 1) + b"12345678\0" + struct.pack("<H", caps & 65535) + b"\x2d\x02\x00" + struct.pack("<H", caps >> 16) + b"\x15" + bytes(10) + b"abcdefghijkl\0mysql_native_password\0"
        ok = bytes([0,0,0,2,0,0,0])
        try:
            self.packet(0, hello); auth = self.read()
            expected = getattr(self.server, 'expected_password', None)
            if expected is not None:
                accepted = False
                try:
                    end=auth.index(0,32);user=auth[32:end].decode();length=auth[end+1];response=auth[end+2:end+2+length]
                    first=hashlib.sha1(expected.encode()).digest();second=hashlib.sha1(first).digest();third=hashlib.sha1(b'12345678abcdefghijkl'+second).digest()
                    scramble=bytes(a^b for a,b in zip(first,third)) if expected else b''
                    accepted=user=='fixture-user' and hmac.compare_digest(response,scramble)
                except (ValueError,IndexError,UnicodeError): pass
                if not accepted:
                    self.server.auth_rejected+=1
                    self.packet(2,b'\xff'+struct.pack('<H',1045)+b'#28000fixture denied');return
                self.server.auth_success+=1
            time.sleep(getattr(self.server, 'auth_delay', 0))
            if getattr(self.server, 'reject_auth', False):
                self.packet(2, b'\xff' + struct.pack('<H',1045) + b'#28000fixture denied'); return
            forced=getattr(self.server,'forced_error',0)
            if forced:
                raw=b'fixture-sensitive-server-error '+str(expected or '').encode()
                self.packet(2,b'\xff'+struct.pack('<H',forced)+b'#42000'+raw);return
            self.packet(2, ok)
            while True:
                request = self.read()
                if request[0] == 1:
                    return
                if request[0] == 25:
                    continue
                if request[0] == 22:
                    self.packet(1, bytes([0,42,0,0,0,1,0,0,0,0,0,0]))
                    self.packet(2, self.column("value")); self.packet(3, bytes([254,0,0,2,0])); continue
                assert request[0] == 3
                sql = request[1:].decode()
                self.server.trace.append(sql)
                if "999" in sql:
                    self.read(); return
                if sql.startswith("SET ") or sql in ("START TRANSACTION READ ONLY", "ROLLBACK", "COMMIT"):
                    self.packet(1, ok)
                elif sql == "SELECT 1" and getattr(self.server,'single_probe_row',False):
                    self.result(["value"],[["1"]])
                elif "VERSION()" in sql:
                    self.result(["version"], [["8.0.36-desktop-fixture"]])
                elif sql == "SHOW DATABASES":
                    self.result(["Database"], [["demo"]])
                elif sql.startswith("SHOW TABLES"):
                    self.result(["Tables_in_demo"], [["items"]])
                elif sql.startswith("SHOW FULL COLUMNS"):
                    self.result(["Field", "Type"], [["id", "varchar(255)"]])
                elif sql.startswith("SHOW INDEX"):
                    self.result(["Key_name"], [["PRIMARY"]])
                elif sql.startswith("EXPLAIN"):
                    self.result(["plan"], [["fixture table scan"]])
                elif "FROM empty" in sql:
                    self.result(["value"], [])
                else:
                    self.result(["value"], [["=formula"], ["中文😀"]] + [[f"row-{n:03d}"] for n in range(2, 250)])
        except (EOFError, OSError, ValueError):
            return


class MysqlServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    def __init__(self):
        self.trace = []
        self.auth_success=0;self.auth_rejected=0
        super().__init__(("127.0.0.1", 0), Mysql)


class Kube(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *args): pass
    def do_GET(self):
        expected=getattr(self.server,'expected_token',None)
        if expected is not None and not hmac.compare_digest(self.headers.get('Authorization',''),'Bearer '+expected):
            self.server.auth_rejected+=1
            self.send_response(401);self.send_header('Content-Length','0');self.end_headers();return
        self.server.auth_success+=1;self.server.trace.append('GET '+self.path)
        if not hasattr(self.server,'accepts'):self.server.accepts=[]
        self.server.accepts.append({'path':self.path,'accept':self.headers.get('Accept','')})
        uri=urlsplit(self.path);query=parse_qs(uri.query);path=uri.path
        namespace=path.split('/namespaces/',1)[1].split('/',1)[0] if '/namespaces/' in path else ''
        def send(value,status=200):
            body=value if isinstance(value,bytes) else json.dumps(value,ensure_ascii=False).encode()
            self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers()
            try:self.wfile.write(body)
            except (BrokenPipeError,ConnectionResetError):pass
        def pod(name='api',phase='Running',ns=None,rv='43'):
            return {'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'namespace':ns or namespace or 'ops','resourceVersion':rv},'spec':{'containers':[{'name':'app'}]},'status':{'phase':phase}}
        crds=[{'apiVersion':'apiextensions.k8s.io/v1','kind':'CustomResourceDefinition','metadata':{'name':'widgets.a.example'},'spec':{'group':'a.example','names':{'plural':'widgets','kind':'Widget'},'scope':'Namespaced','versions':[{'name':'v1','served':True},{'name':'v2','served':True}]}},
              {'apiVersion':'apiextensions.k8s.io/v1','kind':'CustomResourceDefinition','metadata':{'name':'widgets.b.example'},'spec':{'group':'b.example','names':{'plural':'widgets','kind':'Widget'},'scope':'Cluster','versions':[{'name':'v1','served':True}]}}]
        if namespace=='denied':time.sleep(getattr(self.server,'denied_delay',0));send(b'fixture-sensitive-api-error',403);return
        if namespace=='headers-error' and getattr(self.server,'header_error',0):
            self.send_response(self.server.header_error);self.send_header('Content-Length',str(8*1024*1024));self.end_headers();self.wfile.flush();self.request.settimeout(35)
            try:
                if not self.request.recv(1):self.server.header_error_closed=getattr(self.server,'header_error_closed',0)+1
            except (OSError,TimeoutError):pass
            self.close_connection=True;return
        if path.startswith('/apis/apiextensions.k8s.io/v1/customresourcedefinitions'):
            if getattr(self.server,'deny_crd',False):send(b'fixture-sensitive-api-error',403)
            elif path.endswith('/customresourcedefinitions'):send({'metadata':{},'items':crds})
            else:send(crds[0])
            return
        if path.endswith('/log'):
            self.send_response(200);self.send_header('Content-Type','text/plain');self.send_header('Connection','close');self.end_headers()
            try:
                for n in range(100):self.wfile.write(f'2026-11-11T01:00:00Z 容器日志😀 line-{n}\n'.encode());self.wfile.flush();time.sleep(.1)
            except (BrokenPipeError,ConnectionResetError):pass
            self.close_connection=True;return
        if path=='/api/v1/namespaces':
            if getattr(self.server,'deny_namespaces',False):send(b'fixture-sensitive-api-error',403)
            else:send({'metadata':{},'items':[{'metadata':{'name':'ops'}},{'metadata':{'name':'review'}}]})
            return
        if path=='/api/v1/pods' and getattr(self.server,'deny_all',False):send(b'fixture-sensitive-api-error',403);return
        if '/widgets' in path:
            group='b.example' if '/b.example/' in path else 'a.example';version='v2' if '/v2/' in path else 'v1'
            row={'apiVersion':group+'/'+version,'kind':'Widget','metadata':{'name':'widget-sample','namespace':namespace} if namespace else {'name':'widget-sample'},'status':{'phase':'Running'}}
            send({'metadata':{},'items':[row]} if path.endswith('/widgets') else row);return
        if '/secrets' in path:
            row={'apiVersion':'v1','kind':'Secret','metadata':{'name':'secret-sample','namespace':namespace or 'ops','annotations':{'private':'fixture-sensitive-secret'}},'data':{'password':'fixture-sensitive-secret'}}
            send({'metadata':{},'items':[row]} if path.endswith('/secrets') else row);return
        if path.endswith('/pods/api') or path.endswith('/pods/ready') or path.endswith('/pods/failed') or path.endswith('/pods/late-only') or path.endswith('/pods/cross-target'):send(pod(path.rsplit('/',1)[-1]));return
        if 'watch' in query:
            time.sleep(.25);send((json.dumps({'type':'MODIFIED','object':pod()},ensure_ascii=False)+'\n').encode());return
        if 'continue' in query and getattr(self.server,'expire_page_once',False):
            self.server.expire_page_once=False;send(b'fixture-expired-page',410);return
        forced=getattr(self.server,'force_list_error',0)
        if forced:send(b'fixture-sensitive-api-error',forced);return
        self.server.active_lists=getattr(self.server,'active_lists',0)+1;self.server.peak_lists=max(getattr(self.server,'peak_lists',0),self.server.active_lists)
        try:
            time.sleep(getattr(self.server,'list_delay',0))
            page=query.get('continue',[''])[0];limit=max(1,min(200,int(query.get('limit',['100'])[0])))
            if getattr(self.server,'many_search_rows',False):
                start=100 if page=='page2' else 200 if page=='page3' else 0;end=min(start+100,250)
                rows=[pod(f'row-{n+1}','Running',namespace or 'ops') for n in range(start,end)];next_page='page2' if start==0 else 'page3' if start==100 else ''
            elif page=='page2':
                rows=[pod('api-2','Pending',namespace)] if namespace else [pod('api-2','Pending','ops'),pod('api-2','Pending','review')]
                if not namespace or namespace=='review':rows.append(pod('cross-target','Running','review'))
                next_page='page3'
            elif page=='page3':rows=[pod('late-only','Succeeded',namespace)] if namespace else [pod('late-only','Succeeded','ops'),pod('late-only','Succeeded','review')];next_page=''
            else:
                rows=[pod('api','Pending',namespace or 'ops'),pod('ready','Running',namespace or 'review'),pod('failed','Failed',namespace or 'ops')];next_page='page2'
            send({'metadata':{'resourceVersion':'42','continue':next_page},'items':rows[:limit]})
        finally:self.server.active_lists-=1


def start(expected_password=None,expected_token=None):
    db = MysqlServer();db.expected_password=expected_password
    kube = ThreadingHTTPServer(("127.0.0.1", 0), Kube); kube.trace = []; kube.daemon_threads = True;kube.expected_token=expected_token;kube.auth_success=0;kube.auth_rejected=0
    for service in (db, kube):
        threading.Thread(target=service.serve_forever, daemon=True).start()
    return db, kube



def start_tls_mysql(directory, password, shutdown_delay_seconds=0):
    """Synthetic TLS MySQL peer; verifies the client really upgrades its transport."""
    import ssl
    import subprocess
    directory.mkdir(mode=0o700)
    key=directory/'fixture.key';cert=directory/'fixture.crt'
    subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(cert),'-days','1','-subj','/CN=localhost'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(cert,key)
    class TlsMysql(Mysql):
        def handle(self):
            super().handle()
            if shutdown_delay_seconds:
                time.sleep(shutdown_delay_seconds)
        def packet(self, sequence, body):
            if sequence==0:
                body=bytearray(body);offset=body.index(0,1)+1+4+8+1
                capabilities=int.from_bytes(body[offset:offset+2],'little')|2048
                body[offset:offset+2]=capabilities.to_bytes(2,'little')
            elif getattr(self,'tls_auth_pending',False):
                sequence+=1;self.tls_auth_pending=False
            super().packet(sequence,body)
        def read(self):
            if not getattr(self,'tls_active',False):
                request=super().read()
                if len(request)!=32 or not int.from_bytes(request[:4],'little')&2048:
                    raise EOFError('expected TLS upgrade before authentication')
                self.request=context.wrap_socket(self.request,server_side=True)
                self.tls_active=True;self.tls_auth_pending=True;self.server.tls_connections+=1
            return super().read()
    server=socketserver.ThreadingTCPServer(('127.0.0.1',0),TlsMysql)
    server.daemon_threads=True;server.expected_password=password;server.auth_success=0;server.auth_rejected=0;server.trace=[];server.tls_connections=0;server.single_probe_row=True
    threading.Thread(target=server.serve_forever,daemon=True).start()
    return server
