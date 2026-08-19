#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Robust patch: derive old strings directly from the file content (no escape guessing)."""
import io, os
os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
p = 'src/web.rs'
s = io.open(p, encoding='utf-8').read()

# ---------- A: uploadF + handlers ----------
oldA = ("function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;"
       "var fd=new FormData();fd.append('file',f,f.name);"
       "fetch('/api/v1/files/upload',{method:'POST',body:fd}).then(loadF)};loadF()</script>")
newA = "function uploadF(){var f=document.getElementById('fu').files[0];if(!f)return;"
newA += "var fd=new FormData();fd.append('file',f,f.name);"
newA += "fetch('/api/v1/files/upload',{method:'POST',body:fd})"
newA += ".then(function(){document.getElementById('fu').value='';loadF()});}"
newA += "function afItem(n){var m=document.getElementById('addFileMenu');m.classList.remove('open');"
newA += "if(n===1){document.getElementById('fu').click()}else{document.getElementById('folderName').value='';"
newA += "document.getElementById('newFolderModal').classList.add('show')}};"
newA += "function createFolder(){var n=document.getElementById('folderName').value.trim();"
newA += "if(!n){alert('Folder name is required');return}"
newA += "fetch('/api/v1/files/folder',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({name:n})})"
newA += ".then(r=>r.json()).then(d=>{if(d.code===0){document.getElementById('newFolderModal').classList.remove('show');loadF()}"
newA += "else{alert('Error: '+(d.message||'unknown'))}});"
newA += "document.getElementById('addFileBtn').addEventListener('click',function(e){e.stopPropagation();"
newA += "document.getElementById('addFileMenu').classList.toggle('open')});"
newA += "document.addEventListener('click',function(e){var m=document.getElementById('addFileMenu');"
newA += "if(m&&!m.contains(e.target)&&e.target.id!=='addFileBtn'){m.classList.remove('open')}});loadF()</script>"
assert oldA in s, 'A missing'
s = s.replace(oldA, newA)

# ---------- B: row head (folder icon/size/type) ----------
i = s.find('files.forEach(function(f)')
j = s.find("h+='");
assert j > 0, 'row head anchor missing'
# Simpler: oldB spans from j to the end of the file_type td cell.
end = s.find("<td style=color:", j)
oldB = s[j:end]
# Transform oldB -> newB (escape-safe construction)
old_tail = "+f.name+'</td><td>'+(f.size/1024).toFixed(0)+'KB</td><td class=co>'+f.file_type+'"
new_tail = "+'(isF?'📁 ':')+f.name+'</td><td>'+(isF?'-':(f.size/1024).toFixed(0)+'KB')+'</td><td class=co>'+(isF?'folder':f.file_type)+'"
assert old_tail in oldB, "old_tail mismatch"
newB = "var isF=(f.file_type===''folder);" + oldB.replace(old_tail, new_tail)
assert newB != oldB, 'B transform failed'
assert oldB in s, 'B missing'
s = s.replace(oldB, newB)

# ---------- C: actions block ----------
i = s.find('files.forEach(function(f)')
j = s.find("<td><a class=btn btn-o btn-sm href=/api/v1/files/'", i)
assert j > 0, 'actions anchor missing'
k = s.find(">Del</button></td>'", j)
assert k > 0
oldC = s[j:k + len("</td>'")]
newC = ("<td>'" +
       "(isF?'<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button>':'" +
       "<a class=btn btn-o btn-sm href=/api/v1/files/'+f.id+'/download target=_blank>Download</a> " +
       "<button class=btn btn-o btn-sm onclick=linkFile('+f.id+') title=Link>\U0001F517 KB</button> " +
       "<button class=btn btn-o btn-sm onclick=renameF('+f.id+',&quot;'+f.name.replace(/\\\"/g,'')+'&quot;)>Rename</button> " +
       "<button class=btn btn-o btn-sm onclick=copyF('+f.id+')>Copy</button> ')" +
       "<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button></td>')
assert oldC in s, 'C missing'
s = s.replace(oldC, newC)

io.open(p, 'w', encoding='utf-8').write(s)
print('ok: 3 replacements applied')

