import io, subprocess, re
s = io.open('/tmp/loadf_check.js', encoding='utf-8').read()
i = s.find("h+='")
stmt = s[i:1150]
pre = "var h='';var f={name:'x',id:1,file_type:'folder',size:10};var isF=true;var ds='';"

def ok(e):
    frag = stmt[:e]
    if not frag.endswith(';'):
        frag += ';'
    io.open('/tmp/bt3.js', 'w', encoding='utf-8').write(pre + frag)
    r = subprocess.run(['node', '--check', '/tmp/bt3.js'], capture_output=True, text=True)
    return r.returncode == 0

# boundaries: after closing quotes (end of string tokens) and after f.xxx identifiers
ends = []
for m in re.finditer(r"'(?:[^'\\]|\\.)*'", stmt):
    ends.append(m.end())
ends = sorted(set(ends))
last_ok = 0
for e in ends:
    if e > len(stmt):
        continue
    if ok(e):
        last_ok = e
    else:
        print('first failing boundary:', e)
        print('tail:', repr(stmt[max(0, e - 90):e]))
        print('next:', repr(stmt[e:e + 90]))
        break
print('last ok boundary:', last_ok)

