import io, subprocess
s = io.open('/tmp/loadf_check.js', encoding='utf-8').read()
i = s.find("h+='")
stmt = s[i:1150]   # h+='...';  (ends with ; at 1149)
Q = chr(39)
t = 'var h=' + Q + Q + ';var f={name:' + Q + 'x' + Q + ',id:1,file_type:' + Q + 'folder' + Q + ',size:10};var isF=true;var ds=' + Q + Q + ';' + stmt + '\n'
io.open('/tmp/btb.js', 'w', encoding='utf-8').write(t)
r = subprocess.run(['node', '--check', '/tmp/btb.js'], capture_output=True, text=True)
print('OK' if r.returncode == 0 else 'ERR: ' + r.stderr.split(chr(10))[0][:200])

