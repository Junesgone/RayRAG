import io, subprocess
s = io.open('/tmp/bt8.js', encoding='utf-8').read()
k = s.find("':'<a class")
seg = s[k+2:]
if seg.endswith('}'):
    seg = seg[:-1]
Q = chr(39)
t = 'var f={name:' + Q + 'x' + Q + ',id:1,file_type:' + Q + 'folder' + Q + ',size:10};var x=' + Q + 'a' + Q + ';' + seg + '\n'
io.open('/tmp/bt9.js', 'w', encoding='utf-8').write(t)
r = subprocess.run(['node', '--check', '/tmp/bt9.js'], capture_output=True, text=True)
print('OK' if r.returncode == 0 else 'ERR: ' + r.stderr.split(chr(10))[0][:150])
print(repr(seg[-80:]))

