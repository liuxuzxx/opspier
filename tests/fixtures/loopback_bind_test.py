"""Run only in a disposable subprocess with this fixture's LD_PRELOAD enabled."""
import errno,socket
with socket.socket() as s:
 s.bind(('0.0.0.0',0));s.listen(1);assert s.getsockname()[0]=='127.0.0.1'
 with socket.create_connection(s.getsockname()) as client:
  peer,_=s.accept();peer.sendall(b'local');assert client.recv(5)==b'local';peer.close()
with socket.socket() as s:
 try:s.bind(('203.0.113.1',0))
 except OSError as e:assert e.errno==errno.EACCES
 else:raise AssertionError('Non-loopback fixture bind was accepted')
with socket.socket(socket.AF_INET6) as s:
 s.bind(('::',0));s.listen(1);assert s.getsockname()[0]=='::1'
 with socket.create_connection(s.getsockname()[:2]) as client:
  peer,_=s.accept();peer.sendall(b'local');assert client.recv(5)==b'local';peer.close()
print('Owned fixture bind guard: IPv4/IPv6 wildcard -> loopback; non-loopback refused')
