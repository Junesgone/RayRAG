import io, subprocess
s = io.open('/tmp/t8.js', encoding='utf-8').read()
i = s.find("h+='")
stmt = s[i:]
# candidate end positions: right after each closing quote of a complete string token
# simpler: test at each ' position
cands = []
pos = 0
while True:
    q = stmt.find("'", pos)
    if q < 0:
        break
    cands.append(q)
    pos = q + 1
# test from the end backwards: find the largest prefix that parses
ok = None
for e in reversed(cands):
    frag = stmt[:e+1]
    io.open('/tmp/bisect.js', 'w', encoding='utf-8').write("var h='';var f={id:1,name:'x'};var isF=true;var ds='';" + frag + ';\n')
    r = subprocess.run(['node', '--check', '/tmp/bisect.js'], capture_output=True, text=True)
    if r.returncode == 0:
        ok = e
        break
print('largest ok prefix end:', ok)
if ok is not None:
    print('ok tail:', repr(stmt[ok-40:ok+1]))
    print('break tail:', repr(stmt[ok+1:ok+60]))

