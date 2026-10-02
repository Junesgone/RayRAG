# RayRAG UI-traversal preview fixtures generator (internal tooling; not published).
# Contract enforced by scripts/ui-traversal/document-preview-matrix-cdp.mjs:
#   notes.txt  : contains 'preview probe'
#   notes.md   : '# Heading One', 'preview probe', **bold**, [link], 2 list items,
#                blockquote, fenced code, GFM table with 1 body row
#   table.csv  : headers 'name,size'; SECOND data row has an empty size (renders '-')
#   pixel.png  : 2px wide (matrix asserts width === 2)
#   book.xlsx  : header row 'name,size' + one data row
#   deck.pptx  : TWO slides; first slide text 'First slide preview probe'
#   legacy.docx: Word document with 'preview probe' (matrix expects the pending notice)
import os
import struct
import zipfile
import zlib
from pathlib import Path

# Contract path shared with document-preview-matrix-cdp.mjs; override with
# RAYRAG_PREVIEW_FIXTURES. The directory is created owner-only because it holds
# generated test bytes only.
D = Path(os.environ.get('RAYRAG_PREVIEW_FIXTURES', '/tmp/rr-preview-fixtures'))
D.mkdir(parents=True, exist_ok=True)
os.chmod(D, 0o700)

(D / 'notes.txt').write_text('preview probe\n\nRayRAG preview fixture note.\n')
(D / 'notes.md').write_text(
    '# Heading One\n\npreview probe paragraph with **bold** text and a [link](https://example.com).\n\n'
    '- item one\n- item two\n\n> quoted line\n\n```python\nprint("preview probe")\n```\n\n'
    '| name | value |\n|---|---|\n| rayrag | ok |\n'
)
(D / 'table.csv').write_text('name,size\nrayrag,1024\npreview probe,\n')

w = h = 2
raw = b''.join(b'\x00' + b'\xff\x00\x00' * w for _ in range(h))
def chunk(tag, data):
    return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')
(D / 'pixel.png').write_bytes(png)

CT = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>'
RELS = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
DOC = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>preview probe</w:t></w:r></w:p></w:body></w:document>'
with zipfile.ZipFile(D / 'book.docx', 'w') as z:
    z.writestr('[Content_Types].xml', CT)
    z.writestr('_rels/.rels', RELS + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>')
    z.writestr('word/document.xml', DOC)
(D / 'legacy.docx').write_bytes((D / 'book.docx').read_bytes())
WBSHEET = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>name</t></is></c><c r="B1" t="inlineStr"><is><t>size</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>preview probe</t></is></c><c r="B2"><v>1024</v></c></row></sheetData></worksheet>'
with zipfile.ZipFile(D / 'book.xlsx', 'w') as z:
    z.writestr('[Content_Types].xml', CT)
    z.writestr('_rels/.rels', RELS + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>')
    z.writestr('xl/workbook.xml', '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>')
    z.writestr('xl/_rels/workbook.xml.rels', RELS + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>')
    z.writestr('xl/worksheets/sheet1.xml', WBSHEET)
SLIDE = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>'
with zipfile.ZipFile(D / 'deck.pptx', 'w') as z:
    z.writestr('[Content_Types].xml', CT)
    z.writestr('_rels/.rels', RELS + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>')
    z.writestr('ppt/presentation.xml', '<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/><p:sldId id="257" r:id="rId2"/></p:sldIdLst></p:presentation>')
    z.writestr('ppt/_rels/presentation.xml.rels', RELS + '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>')
    z.writestr('ppt/slides/slide1.xml', SLIDE.replace('{text}', 'First slide preview probe'))
    z.writestr('ppt/slides/slide2.xml', SLIDE.replace('{text}', 'Second slide'))

print('preview fixtures written to', D)
