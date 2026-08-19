import io, subprocess
s = io.open('/tmp/btc.js', encoding='utf-8').read()
# statement starts at h+=
i = s.find("h+=" )
stmt = s[i:]   # ends with ;
def ok(e):
    frag = stmt[:e]
    if not frag.endswith(';'):
        frag += ';'
    io.open('/tmp/bt3.js', 'w', encoding='utf-8').write('var f={name:"x",id:1,file_type:"folder",size:10};var isF=true;var ds="";' + frag)
    r = subprocess.run(['node', '--check', '/tmp/bt3.js'], capture_output=True, text=True)
    return r.returncode == 0
# candidate boundaries: after each complete string token or identifier
import re
ends = set()
for m in re.finditer(r"'(?:[^'\\]|\\.)*'", stmt):
    ends.add(m.end())
for m in re.finditer(r'\b(?:f\.(?:name|id|file_type|size)|ds|isF)\b', stmt):
    ends.add(m.end())
for m in re.finditer(r'\)', stmt):
    ends.add(m.end())
ends = sorted(e for e in ends if e <= len(stmt))
last_ok = 0
for e in ends:
    if ok(e):
        last_ok = e
    else:
        print('first failing boundary:', e)
        print('tail:', repr(stmt[max(0, e - 100):e]))
        print('next:', repr(stmt[e:e + 100]))
        break
print('last ok:', last_ok, repr(stmt[max(0, last_ok - 60):last_ok]))

