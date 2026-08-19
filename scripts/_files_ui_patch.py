#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Patch files_page JS in src/web.rs: upload reset + Add-file dropdown handlers + folder rows."""
import io, os
os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
p = 'src/web.rs'
s = io.open(p, encoding='utf-8').read()

# --- A: uploadF reset + dropdown handlers ---
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

# --- B: row head with folder icon/size/type ---
oldB = ("h+='<tr data-name=\\\\''+f.name.toLowerCase()+'\\'><td><input type=checkbox class=fileCk value='+f.id+'></td>"
       "<td style=font-weight:500>'+f.name+'</td><td>'"
       "+(f.size/1024).toFixed(0)+'KB</td><td class=co>'+f.file_type+'</td>")
newB = ("var isF=(f.file_type==='folder');"
       "h+='<tr data-name=\\\\''+f.name.toLowerCase()+'\\'><td><input type=checkbox class=fileCk value='+f.id+'></td>"
       "<td style=font-weight:500>'"
       "+(isF?'\U0001F4C1 ':'')+f.name+'</td><td>'"
       "+(isF?'-':(f.size/1024).toFixed(0)+'KB')+'</td><td class=co>'"
       "+(isF?'folder':f.file_type)+'</td>")
assert oldB in s, 'B missing'
s = s.replace(oldB, newB)

# --- C: actions block ---
oldC = ("<td><a class=btn btn-o btn-sm href=/api/v1/files/'+f.id+'/download target=_blank>Download</a> "
       "<button class=btn btn-o btn-sm onclick=linkFile('+f.id+') title=Link>\U0001F517 KB</button> "
       "<button class=btn btn-o btn-sm onclick=renameF('+f.id+',&quot;'+f.name.replace(/\\\"/g,'')+'&quot;)>Rename</button> "
       "<button class=btn btn-o btn-sm onclick=copyF('+f.id+')>Copy</button> "
       "<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button></td>")
newC = ("<td>'"
       + "(isF?'<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button>':'"
       + "<a class=btn btn-o btn-sm href=/api/v1/files/'+f.id+'/download target=_blank>Download</a> "
       "<button class=btn btn-o btn-sm onclick=linkFile('+f.id+') title=Link>\U0001F517 KB</button> "
       "<button class=btn btn-o btn-sm onclick=renameF('+f.id+',&quot;'+f.name.replace(/\\\"/g,'')+'&quot;)>Rename</button> "
       "<button class=btn btn-o btn-sm onclick=copyF('+f.id+')>Copy</button> ')"
       + "<button class=btn btn-d btn-sm onclick=delFile('+f.id+')>Del</button></td>")
assert oldC in s, 'C missing'
s = s.replace(oldC, newC)

io.open(p, 'w', encoding='utf-8').write(s)
print('ok: 3 replacements applied')

