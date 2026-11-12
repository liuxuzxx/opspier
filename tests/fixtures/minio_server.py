"""Owned MinIO server and an independent SigV4 client for desktop E2E.

Set OPSPIER_MINIO_SERVER to a locally built official MinIO binary. This fixture
binds loopback only, uses random credentials in child environment, and removes
its own temporary data. It never uses an existing MinIO service or config.
"""
from datetime import datetime,timezone
import hashlib,hmac,os,secrets,shutil,socket,subprocess,tempfile,time
from pathlib import Path
import urllib.request,urllib.error,urllib.parse

class MinioFixture:
    def __init__(self,binary):
        if not Path(binary).is_file():raise RuntimeError('local MinIO test binary unavailable')
        self.directory=Path(tempfile.mkdtemp(prefix='opspier-s3-fixture-'))
        self.access='fixture'+secrets.token_hex(10);self.secret=secrets.token_hex(32)
        def port():
            with socket.socket() as stream:stream.bind(('127.0.0.1',0));return stream.getsockname()[1]
        self.host='127.0.0.1:'+str(port());self.endpoint='http://'+self.host
        env={key:os.environ[key] for key in ['PATH','LANG','LC_ALL','LD_LIBRARY_PATH'] if key in os.environ}
        env.update(HOME=str(self.directory),MINIO_ROOT_USER=self.access,MINIO_ROOT_PASSWORD=self.secret,MINIO_BROWSER='off',MINIO_CI_CD='1',GOMAXPROCS='2')
        self.process=subprocess.Popen([binary,'server',str(self.directory/'data'),'--address',self.host,'--console-address','127.0.0.1:'+str(port())],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
        try:
            deadline=time.monotonic()+30
            while True:
                try:
                    urllib.request.urlopen(self.endpoint+'/minio/health/ready',timeout=1).close();break
                except (OSError,urllib.error.URLError):
                    if self.process.poll() is not None or time.monotonic()>deadline:raise RuntimeError('owned MinIO server did not become ready') from None
                    time.sleep(.1)
            self.request('PUT','/fixture-bucket');self.request('PUT','/second-bucket')
            self.request('PUT','/fixture-bucket/folder/hello.txt',b'hello from actual MinIO')
            for number in range(205):self.request('PUT',f'/fixture-bucket/page-{number:03}.txt',str(number).encode())
        except BaseException:self.close();raise
    def request(self,method,path,data=b'',query=None):
        stamp=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ');day=stamp[:8]
        uri=urllib.parse.quote(path,safe='/~');query_text=urllib.parse.urlencode(sorted((query or {}).items()),quote_via=urllib.parse.quote)
        digest=hashlib.sha256(data).hexdigest();headers={'host':self.host,'x-amz-content-sha256':digest,'x-amz-date':stamp}
        signed='host;x-amz-content-sha256;x-amz-date'
        canonical='\n'.join([method,uri,query_text,''.join(key+':'+headers[key]+'\n' for key in sorted(headers)),signed,digest])
        scope=day+'/us-east-1/s3/aws4_request';sign='AWS4-HMAC-SHA256\n'+stamp+'\n'+scope+'\n'+hashlib.sha256(canonical.encode()).hexdigest()
        def mac(key,value):return hmac.new(key,value.encode(),hashlib.sha256).digest()
        key=mac(mac(mac(mac(('AWS4'+self.secret).encode(),day),'us-east-1'),'s3'),'aws4_request')
        headers['Authorization']='AWS4-HMAC-SHA256 Credential='+self.access+'/'+scope+', SignedHeaders='+signed+', Signature='+hmac.new(key,sign.encode(),hashlib.sha256).hexdigest()
        request=urllib.request.Request(self.endpoint+uri+('?' + query_text if query_text else ''),data=data if method in ('PUT','POST') else None,headers=headers,method=method)
        try:
            with urllib.request.urlopen(request,timeout=30) as response:return response.read()
        except urllib.error.HTTPError as error:raise RuntimeError('fixture S3 HTTP '+str(error.code)) from None
    def exists(self,key):
        try:self.request('HEAD','/fixture-bucket/'+key);return True
        except RuntimeError as error:
            if str(error)=='fixture S3 HTTP 404':return False
            raise
    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:self.process.wait(timeout=8)
            except subprocess.TimeoutExpired:self.process.kill();self.process.wait()
        shutil.rmtree(self.directory,ignore_errors=False)
