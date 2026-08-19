import io
p = '/home/junesgone/.openclaw/workspace/RayRAG/scripts/_files_ui_patch3.py'
s = io.open(p, encoding='utf-8').read()
Q = chr(39)
BS = chr(92)
EMO = "\U0001F517"
L = list
ch = []
def A(t):
    ch.extend(L(t))
A('<td>' + Q)
A('+(isF?' + '<button class=btn btn-d btn-sm onclick=delFile(' + Q)
A('+f.id+')
A(Q + ')>Del</button>' + Q + ':' + Q)
A('<a class=btn btn-o btn-sm href=/api/v1/files/')
A('+f.id+')
A('/download target=_blank>Download</a> <button class=btn btn-o btn-sm onclick=linkFile(' + Q)
A('+f.id+')
A(Q + ') title=Link>' + EMO + ' KB</button> <button class=btn btn-o btn-sm onclick=renameF(' + Q)
A('+f.id+')
A(Q + Q + ',&quot;' + Q)
A('+f.name.replace(/' + BS + '"' + '/g,' + Q + Q + ')+&quot;)>Rename</button> <button class=btn btn-o btn-sm onclick=copyF(' + Q)
A('+f.id+')
A(Q + Q + ')>Copy</button> ')
A(')')
A(Q + '</td></tr>' + Q)
text = ''.join(ch)
block = 'newC = ' + repr(text) + '\n'
start = s.find('newC = ')
end = s.find('assert oldC in s')
s = s[:start] + block + s[end:]
io.open(p, 'w', encoding='utf-8').write(s)
print(text)

