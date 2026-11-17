"""Real kubectl + built OpsPier supervisor on an owned PTY and API peer."""
import argparse,fcntl,json,os,pty,select,shlex,signal,struct,tempfile,time,termios,uuid
from pathlib import Path
from fixtures.kubectl_api import Api
class Host:
    def __init__(self,binary,root,server):
        self.directory=root/('terminal-'+uuid.uuid4().hex);self.directory.mkdir(mode=0o700)
        self.config=server.config();self.path=self.directory/'kubeconfig';self.save('')
        rc=self.directory/'init.bash';rc.write_text("unset HISTFILE\nHISTFILESIZE=0\nPS1='kubectl> '\n");rc.chmod(0o600)
        lock=self.directory/'.alive.lock';lock.touch(mode=0o600)
        self.pid,self.fd=pty.fork()
        if self.pid==0:
            env={k:v for k,v in os.environ.items()if k not in ['BASH_ENV','ENV','HISTFILE','PROMPT_COMMAND','SHELLOPTS','BASHOPTS'] and not k.startswith('BASH_FUNC_')};env.update(KUBECONFIG=str(self.path),KUBECTL_REMOTE_COMMAND_WEBSOCKETS='true',TERM='xterm-256color')
            os.execve(str(binary),[str(binary),'--opspier-terminal-supervisor','/bin/bash',str(rc)],env)
        self.pending=b'';self.resize(90,31);self.until('kubectl> ')
    def save(self,namespace):
        context=self.config['contexts'][0]['context'];context.pop('namespace',None)
        if namespace:context['namespace']=namespace
        temporary=self.directory/'next-config';temporary.write_text(json.dumps(self.config));temporary.chmod(0o600);os.replace(temporary,self.path)
    def resize(self,cols,rows):fcntl.ioctl(self.fd,termios.TIOCSWINSZ,struct.pack('HHHH',rows,cols,0,0))
    def send(self,text):os.write(self.fd,text.encode()if isinstance(text,str)else text)
    def until(self,marker,timeout=12):
        wanted=marker.encode();deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            index=self.pending.find(wanted)
            if index>=0:result=self.pending[:index+len(wanted)];self.pending=self.pending[index+len(wanted):];return result.decode(errors='replace')
            if select.select([self.fd],[],[],.1)[0]:
                try:self.pending+=os.read(self.fd,65536)
                except OSError:break
        raise AssertionError('real kubectl PTY marker absent: '+marker)
    def command(self,text):self.send(text+'\r');return self.until('kubectl> ')
    def close(self):
        os.kill(self.pid,signal.SIGTERM);deadline=time.monotonic()+6
        while time.monotonic()<deadline:
            if os.waitpid(self.pid,os.WNOHANG)[0]:os.close(self.fd);return
            time.sleep(.02)
        raise AssertionError('built supervisor did not exit')
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,required=True);args=parser.parse_args();peers=[Api(label,uuid.uuid4().hex).start()for label in ['alpha','beta']];hosts=[]
    try:
        with tempfile.TemporaryDirectory(prefix='opspier-kubectl-protocol-')as directory:
            root=Path(directory);root.chmod(0o700);hosts=[Host(args.binary,root,peer)for peer in peers];a,b=hosts
            for host,label in [(a,'alpha'),(b,'beta')]:
                output=host.command('kubectl get pods -o name')
                assert 'pod/'+label in output, 'owned kubectl get: '+output.replace(peers[0].token,'[redacted]').replace(peers[1].token,'[redacted]')[:1500]
            description=a.command('kubectl describe pod alpha');assert 'alpha' in description and 'loopback-node' in description
            a.save('second');assert 'pod/alpha' in a.command('kubectl get pods -o name');assert '/api/v1/namespaces/second/pods' in peers[0].requests;a.save('')
            assert 'pod/alpha' in a.command('kubectl get pods -o name');assert '/api/v1/namespaces/default/pods' in peers[0].requests
            a.send('kubectl logs -f alpha\r');a.until('alpha 中文日志');a.send(b'\x03');a.until('kubectl> ')
            a.send('kubectl get pods --watch -o name\r');a.until('pod/alpha');a.send(b'\x03');a.until('kubectl> ')
            a.send('kubectl exec -it alpha -- sh\r');a.until('remote-ready');a.send('中文\r');a.until('remote-中文');a.resize(120,41)
            deadline=time.monotonic()+5
            while (120,41)not in peers[0].resizes and time.monotonic()<deadline:time.sleep(.02)
            assert(120,41)in peers[0].resizes;a.send(b'\x03');a.until('remote-interrupted');a.until('kubectl> ');assert peers[0].exec_unicode and peers[0].exec_interrupt
            assert 'fixture RBAC denied' in a.command('kubectl get pods -n denied');assert peers[0].denied==1
            a.command('kubectl label pod alpha fixture=changed --overwrite');assert getattr(peers[0],'mutated',False)
            assert peers[1].exec_open==0 and not getattr(peers[1],'mutated',False)
            for host in hosts:host.close()
            hosts=[]
        print(json.dumps({'realKubectl':True,'builtSupervisor':True,'sameNamedContextsIsolated':True,'get':True,'describe':True,'namespaceDefaultAndAtomicChange':True,'logsFollowCtrlC':True,'watchCtrlC':True,'execWebsocketUnicodeCtrlC':True,'execResize':True,'RBACDenied':True,'writeCommand':True,'ownedProcessesExited':True}))
    finally:
        for host in hosts:
            try:host.close()
            except (ProcessLookupError,ChildProcessError):pass
        for peer in peers:peer.close()
if __name__=='__main__':main()
