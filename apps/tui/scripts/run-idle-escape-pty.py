import os,pty,subprocess,time,select,sys
for scenario in (sys.argv[1:] or ['idle','active','tree','draft']):
 marker='/tmp/escape-'+scenario
 for suffix in ['.ready','.rows']:
  try: os.unlink(marker+suffix)
  except FileNotFoundError: pass
 master,slave=pty.openpty()
 p=subprocess.Popen(['node','--import','tsx','scripts/idle-escape-pty-smoke.ts',scenario,marker],stdin=slave,stdout=slave,stderr=slave,env={**os.environ,'TERM':'xterm-256color'})
 os.close(slave); out=b''; sent=False; start=time.time()
 while p.poll() is None and time.time()-start<30:
  if select.select([master],[],[],.05)[0]:
   try:out+=os.read(master,65536)
   except OSError:break
  if not sent and os.path.exists(marker+'.ready'):
   if scenario=='draft': os.write(master,b'unsent draft');time.sleep(.1)
   os.write(master,b'\x1b');sent=True
   if scenario=='draft': time.sleep(.35);os.write(master,b'\x1b')
   if scenario=='tree':
    time.sleep(.15);os.write(master,b'\x1b');time.sleep(.4);os.write(master,b'\x1b')
 p.wait(timeout=10)
 open(marker+'.log','wb').write(out)
 print(scenario,p.returncode)
 if p.returncode: print(out[-3000:].decode(errors='replace'));sys.exit(1)
