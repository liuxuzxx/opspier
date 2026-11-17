"""Verify the real kernel listeners owned by a guarded Java fixture, not cached JDK addresses."""
import os,subprocess,sys
from pathlib import Path
probe=Path(__file__).with_name('BindProbe.java')
process=subprocess.Popen(['java','-Djava.net.preferIPv4Stack=true',str(probe)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
try:
 for _ in range(2):
  line=process.stdout.readline().strip();assert line.startswith('PORT='),'Java probe did not expose an owned listener'
  port=int(line.partition('=')[2]);owned=set()
  for fd in (Path('/proc')/str(process.pid)/'fd').iterdir():
   try:
    target=os.readlink(fd)
    if target.startswith('socket:['):owned.add(target[8:-1])
   except OSError:pass
  matches=[]
  for name in ['tcp','tcp6']:
   for row in (Path('/proc')/str(process.pid)/'net'/name).read_text().splitlines()[1:]:
    fields=row.split();address,number=fields[1].split(':')
    if fields[3]=='0A' and int(number,16)==port and fields[9] in owned:matches.append(address)
  assert matches and all(address in ['0100007F','00000000000000000000000001000000'] for address in matches),'Actual Java listener escaped loopback'
  process.stdin.write('\n');process.stdin.flush()
 code=process.wait(timeout=15);assert code==0,'Java listener exchange failed'
 print('Kernel-owned JDK classic/NIO listeners bound only loopback; real local exchange passed')
finally:
 if process.poll() is None:process.kill();process.wait()
