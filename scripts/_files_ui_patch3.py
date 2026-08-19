#!/usr/bin/env python3
# -*- coding: utf-8 -*-
import io, os
os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
p = 'src/web.rs'
s = io.open(p, encoding='utf-8').read()
Q = chr(39)  # single quote
BS = chr(92) # backslash
def L(parts):
    return ''.join(parts)

# ============ A: uploadF + dropdown handlers ============
oldA = ("function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;"
       "var fd=new FormData();fd.append('file',f,f.name);"
       "fetch('/api/v1/files/upload',{method:'POST',body:fd}).then(loadF)};loadF()</script>")
newA = ("function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;"
       "var fd=new FormData();fd.append('file',f,f.name);"
       "fetch('/api/v1/files/upload',{method:'POST',body:fd})"
       ".then(function(){document.getElementById('fu').value='';loadF()});}"
       "function afItem(n){var m=document.getElementById('addFileMenu');m.classList.remove('open');"
       "if(n===1){document.getElementById('fu').click()}else{document.getElementById('folderName').value='';"
       "document.getElementById('newFolderModal').classList.add('show')}};"
       "function createFolder(){var n=document.getElementById('folderName').value.trim();"
       "if(!n){alert('Folder name is required');return}"
       "fetch('/api/v1/files/folder',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})})"
       ".then(r=>r.json()).then(d=>{if(d.code===0){document.getElementById('newFolderModal').classList.remove('show');loadF()}"
       "else{alert('Error: '+(d.message||'unknown'))}});}")
newA += ("document.getElementById('addFileBtn').addEventListener('click',function(e){e.stopPropagation();"
       "document.getElementById('addFileMenu').classList.toggle('open')});"
       "document.addEventListener('click',function(e){var m=document.getElementById('addFileMenu');"
       "if(m&&!m.contains(e.target)&&e.target.id!=='addFileBtn'){m.classList.remove('open')}});loadF()</script>")
assert oldA in s, 'A missing'
s = s.replace(oldA, newA)

# ============ B: row head ============
i = s.find('files.forEach(function(f)')
j = s.find(L(["h+=", Q, "<tr"]), i)
assert j > 0, 'B anchor missing'
end = s.find('<td style=color:', j)
oldB = s[j:end]
old_tail = L([Q, "+", "f.name", "+", Q, "</td><td>", Q, "+", "(", "f.size/1024).toFixed(0)+", Q, "KB</td><td class=co>", Q, "+", "f.file_type", "+", Q, "</td>"])
new_tail = L([Q, "+", "(", "isF?", Q, "\U0001F4C1 ", Q, ":", Q, Q, ")", "+", "f.name", "+", Q, "</td><td>", Q, "+", "(", "isF?", Q, "-</td><td class=co>", Q, ":", "(", "f.size/1024).toFixed(0)+", Q, "KB</td><td class=co>", Q, ")", "+", "(", "isF?", Q, "folder", Q, ":", "f.file_type", ")", "+", Q, "</td>"])
assert old_tail in oldB, 'old_tail mismatch: ' + repr(oldB[:80])
newB = "var isF=(f.file_type==='folder');" + oldB.replace(old_tail, new_tail)
assert newB != oldB
s = s.replace(oldB, newB)

# ============ C: actions block ============
oldC = '</td><td><a class=btn btn-o btn-sm href=/api/v1/files/\'+f.id+\'/download target=_blank>Download</a> <button class=btn btn-o btn-sm onclick=linkFile(\'+f.id+\') title=Link>🔗 KB</button> <button class=btn btn-o btn-sm onclick=renameF(\'+f.id+\',&quot;\'+f.name.replace(/\\"/g,\'\')+\'&quot;)>Rename</button> <button class=btn btn-o btn-sm onclick=copyF(\'+f.id+\')>Copy</button> <button class=btn btn-d btn-sm onclick=delFile(\'+f.id+\')>Del</button></td></tr>\''
newC = '</td><td>\'+(isF?\'<button class=btn btn-d btn-sm onclick=delFile(\'+f.id+\')>Del</button>\':\'<a class=btn btn-o btn-sm href=/api/v1/files/\'+f.id+\'/download target=_blank>Download</a> <button class=btn btn-o btn-sm onclick=linkFile(\'+f.id+\') title=Link>🔗 KB</button> <button class=btn btn-o btn-sm onclick=renameF(\'+f.id+\',&quot;\'+f.name.replace(/\\"/g,\'\')+\'&quot;)>Rename</button> <button class=btn btn-o btn-sm onclick=copyF(\'+f.id+\')>Copy</button> \')+\'</td></tr>\';'
assert s.count(oldC) == 1, 'C block missing'
s = s.replace(oldC, newC)

io.open(p, 'w', encoding='utf-8').write(s)
print('ok: 3 replacements applied')
