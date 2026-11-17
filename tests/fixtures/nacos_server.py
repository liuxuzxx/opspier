"""Owned real Nacos 2.x server; all ports restricted by a preverified test-only bind adapter."""
import base64,json,os,re,secrets,shutil,socket,subprocess,tempfile,time,urllib.error,urllib.parse,urllib.request
from pathlib import Path

def owned_listeners(pid):
    sockets=set()
    for fd in (Path('/proc')/str(pid)/'fd').iterdir():
        try:
            target=os.readlink(fd)
            if target.startswith('socket:['):sockets.add(target[8:-1])
        except OSError:pass
    addresses=[]
    for family in ['tcp','tcp6','udp','udp6']:
        for row in (Path('/proc')/str(pid)/'net'/family).read_text().splitlines()[1:]:
            fields=row.split()
            if fields[9] not in sockets:continue
            if family.startswith('tcp') and fields[3]!='0A':continue
            address=fields[1].split(':')[0]
            addresses.append(address)
    return addresses

class NacosFixture:
    def __init__(self,distribution,guard,password):
        self.distribution=Path(distribution).resolve();self.guard=Path(guard).resolve();self.password=password
        assert (self.distribution/'target/nacos-server.jar').is_file() and self.guard.is_file()
        self.directory=Path(tempfile.mkdtemp(prefix='opspier-nacos-'));self.directory.chmod(0o700);self.process=None;self.log=None;self.token=None
        self.port=self.free_base_port();self.endpoint=f'http://127.0.0.1:{self.port}/nacos'
    @staticmethod
    def free_base_port():
        for _ in range(100):
            base=20000+secrets.randbelow(29000);reserved=[]
            try:
                for port in [base,base-1000,base+1000,base+1001]:
                    stream=socket.socket();stream.bind(('127.0.0.1',port));reserved.append(stream)
                return base
            except OSError:pass
            finally:
                for stream in reserved:stream.close()
        raise RuntimeError('No isolated Nacos port set available')
    def call(self,method,path,parameters=None,token=None):
        body=None;url=self.endpoint+path
        if parameters:
            encoded=urllib.parse.urlencode(parameters).encode()
            if method in ['GET','DELETE']:url+='?'+encoded.decode()
            else:body=encoded
        headers={'Content-Type':'application/x-www-form-urlencoded'}
        if token:headers['Authorization']='Bearer '+token
        request=urllib.request.Request(url,data=body,headers=headers,method=method)
        try:
            with urllib.request.urlopen(request,timeout=5) as response:return response.read()
        except urllib.error.HTTPError as error:raise RuntimeError('Owned Nacos fixture HTTP '+str(error.code)) from None
    def start(self):
        try:
            shutil.copytree(self.distribution/'conf',self.directory/'conf')
            properties=self.directory/'conf/application.properties'
            extra='\nserver.address=127.0.0.1\nserver.port='+str(self.port)+'\nnacos.inetutils.ip-address=127.0.0.1\nnacos.core.auth.enabled=true\nnacos.core.auth.system.type=nacos\nnacos.core.auth.caching.enabled=false\nnacos.core.auth.enable.userAgentAuthWhite=false\nnacos.core.auth.server.identity.key=fixture-identity\nnacos.core.auth.server.identity.value='+secrets.token_hex(32)+'\nnacos.core.auth.plugin.nacos.token.secret.key='+base64.b64encode(secrets.token_bytes(48)).decode()+'\n'
            properties.write_text(properties.read_text()+extra);properties.chmod(0o600)
            self.log=open(self.directory/'server.log','wb');os.chmod(self.directory/'server.log',0o600)
            args=['java','-Xms96m','-Xmx384m','-XX:ActiveProcessorCount=2','-Djava.net.preferIPv4Stack=true','-Dnacos.standalone=true','-Dnacos.home='+str(self.directory),'-jar',str(self.distribution/'target/nacos-server.jar'),'--spring.config.additional-location=file:'+str(self.directory/'conf/')+'/']
            self.process=subprocess.Popen(args,cwd=self.directory,env={**os.environ,'LD_PRELOAD':str(self.guard)},stdout=self.log,stderr=self.log,start_new_session=True)
            deadline=time.monotonic()+100
            while time.monotonic()<deadline:
                if self.process.poll() is not None:raise RuntimeError('Owned Nacos fixture exited during startup')
                listeners=owned_listeners(self.process.pid)
                assert all(a in ['0100007F','00000000000000000000000001000000'] for a in listeners),'Owned Nacos listener escaped loopback'
                try:
                    state=json.loads(self.call('GET','/v1/console/server/state'))
                    if state.get('version','').startswith('2.'):break
                except (RuntimeError,OSError,ValueError):pass
                time.sleep(.3)
            else:raise RuntimeError('Owned Nacos fixture startup deadline exceeded')
            token=json.loads(self.call('POST','/v1/auth/users/login',{'username':'nacos','password':'nacos'}))['accessToken']
            self.call('PUT','/v1/auth/users',{'username':'nacos','newPassword':self.password},token)
            self.token=json.loads(self.call('POST','/v1/auth/users/login',{'username':'nacos','password':self.password}))['accessToken']
            return self
        except Exception as error:
            # Retain only exception class names; server diagnostics may contain configuration values.
            classes=[]
            log=self.directory/'server.log'
            if log.exists():
                classes=sorted(set(re.findall(r'(?:Caused by: |^)((?:java|org|com)\.[A-Za-z0-9_.$]*(?:Exception|Error))',log.read_text(errors='replace'),re.M)))[:12]
            kind=type(error).__name__;self.close()
            raise RuntimeError('Owned Nacos fixture startup failed: '+kind+'; classes='+','.join(classes)) from None
    def close(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        if self.log:self.log.close()
        self.token=None;self.password=None
        shutil.rmtree(self.directory,ignore_errors=False)
    def __enter__(self):return self.start()
    def __exit__(self,*_):self.close()

def main():
    import argparse,sys
    parser=argparse.ArgumentParser();parser.add_argument('--distribution',type=Path,required=True);parser.add_argument('--guard',type=Path,required=True);args=parser.parse_args()
    material=json.loads(sys.stdin.readline());fixture=NacosFixture(args.distribution,args.guard,material['password'])
    with fixture as server:
        server.call('POST','/v1/console/namespaces',{'customNamespaceId':'fixture-blue','namespaceName':'Fixture','namespaceDesc':'Isolated test'},server.token)
        server.call('POST','/v1/cs/configs',{'tenant':'fixture-blue','group':'BLUE','dataId':'file.yaml','content':'initial: value\r\n','type':'yaml','appName':'fixture-app','desc':'fixture-description'},server.token)
        server.call('POST','/v1/ns/service',{'namespaceId':'public','serviceName':'BLUE@@api','protectThreshold':'0.2','metadata':'{"keep":"value"}','selector':'{"type":"none"}'},server.token)
        server.call('POST','/v1/ns/instance',{'namespaceId':'public','serviceName':'BLUE@@api','ip':'127.0.0.1','port':'12345','clusterName':'ALT','ephemeral':'false','enabled':'true','weight':'1','metadata':'{"keep":"value"}'},server.token)
        print(json.dumps({'endpoint':server.endpoint,'pid':server.process.pid}),flush=True)
        sys.stdin.readline()
    print(json.dumps({'cleaned':not fixture.directory.exists(),'exited':fixture.process.poll() is not None}),flush=True)
if __name__=='__main__':main()
