import io, subprocess
s = io.open('/tmp/btc.js', encoding='utf-8').read()
i = s.find("h+=" )
stmt = s[i:]
pre = 'var f={name:"x",id:1,file_type:"folder",size:10};var isF=true;var ds="";'

def ok(e):
    frag = stmt[:e]
    if not frag.endswith(';'):
        frag += ';'
    io.open('/tmp/bt3.js', 'w', encoding='utf-8').write(pre + frag)
    r = subprocess.run(['node', '--check', '/tmp/bt3.js'], capture_output=True, text=True)
    return r.returncode == 0

# walk boundaries forward from 146
import re
cands = []
for m in re.finditer(r"'(?:[^'\\]|\\.)*'", stmt):
    cands.append(m.end())
for m in re.finditer(r'\b(?:f\.(?:name|id|file_type|size)|ds)\b', stmt):
    cands.append(m.end())
for m in re.finditer(r'\)', stmt):
    cands.append(m.end())
cands = sorted(set(c for c in cands if c > 146))
last = 146
for e in cands:
    if e > len(stmt):
        break
    if ok(e):
        last = e
    else:
        print('first failing:', e)
        print('tail:', repr(stmt[max(0, e - 110):e]))
        print('next:', repr(stmt[e:e + 60]))
        break
print('last ok:', last)

