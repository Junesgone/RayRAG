import io
p = '/home/junesgone/.openclaw/workspace/RayRAG/scripts/_files_ui_patch3.py'
s = io.open(p, encoding='utf-8').read()
start = s.find('# ============ C: actions block ===========')
assert start > 0, 'C marker missing'
Q = chr(39)
BS2 = chr(92) + chr(92)
EMO = "\U0001F517"
FID = '+f.id+'
B = ' <button class=btn btn-o btn-sm'
BLK = '<button class=btn btn-d btn-sm'
oldC_parts = [
    '</td><td><a class=btn btn-o btn-sm href=/api/v1/files/' + Q,
    FID,
    Q + '/download target=_blank>Download</a>' + B + ' onclick=linkFile(' + Q,
    FID,
    Q + ') title=Link>' + EMO + ' KB</button>' + B + ' onclick=renameF(' + Q,
    FID,
    Q + ',&quot;' + Q,
    '+f.name.replace(/' + chr(92) + '"' + '/g,' + Q + Q + ')',
    '+' + Q + '&quot;)>Rename</button>' + B + ' onclick=copyF(' + Q,
    FID,
    Q + ')>Copy</button> ' + BLK + ' onclick=delFile(' + Q,
    FID,
    Q + ')>Del</button></td></tr>' + Q,
]
oldC = ''.join(oldC_parts)
newC_parts = [
    '</td><td>' + Q,
    '+(isF?',
    Q + BLK + ' onclick=delFile(' + Q,
    FID,
    Q + ')>Del</button>' + Q + ':',
    Q + '<a class=btn btn-o btn-sm href=/api/v1/files/' + Q,
    FID,
    Q + '/download target=_blank>Download</a>' + B + ' onclick=linkFile(' + Q,
    FID,
    Q + ') title=Link>' + EMO + ' KB</button>' + B + ' onclick=renameF(' + Q,
    FID,
    Q + ',&quot;' + Q,
    '+f.name.replace(/' + chr(92) + '"' + '/g,' + Q + Q + ')',
    '+' + Q + '&quot;)>Rename</button>' + B + ' onclick=copyF(' + Q,
    FID,
    Q + ')>Copy</button> ' + Q,
    ')+',
    Q + '</td></tr>' + Q + ';',
]
newC = ''.join(newC_parts)
block = '# ============ C: actions block ============\n'
block += 'oldC = ' + repr(oldC) + '\n'
block += 'newC = ' + repr(newC) + '\n'
block += "assert s.count(oldC) == 1, 'C block missing'\n"
block += 's = s.replace(oldC, newC)\n'
block += "\nio.open(p, 'w', encoding='utf-8').write(s)\n"
block += "print('ok: 3 replacements applied')\n"
s = s[:start] + block
io.open(p, 'w', encoding='utf-8').write(s)
print('oldC:', oldC[:90])
print('newC:', newC[:90])

