#!/usr/bin/env python3
"""Build the fixtures the document-previewer matrix probe uploads.

`scripts/ui-traversal/document-preview-matrix-cdp.mjs` needs one file per previewer
branch, including a few that no office suite would emit for us (a two-cell workbook
and a two-slide deck are written by hand so the assertions can name exact cell
values). They are generated into a scratch directory and never committed.

Usage:
    python3 scripts/ui-traversal/make-preview-fixtures.py /tmp/rr-preview-fixtures
"""
import os
import struct
import sys
import zipfile
import zlib

NS = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
RNS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def write_text(root):
    with open(os.path.join(root, "notes.txt"), "w", encoding="utf-8") as handle:
        handle.write("RayRAG preview probe\nline two\nline three\n")
    with open(os.path.join(root, "notes.md"), "w", encoding="utf-8") as handle:
        handle.write(
            "# Heading One\n\n"
            "Some **bold** text and a [link](https://example.com).\n\n"
            "- item one\n- item two\n\n"
            "| a | b |\n| --- | --- |\n| 1 | 2 |\n\n"
            "```rust\nfn main() {}\n```\n\n"
            "> quoted line\n"
        )
    with open(os.path.join(root, "table.csv"), "w", encoding="utf-8") as handle:
        handle.write('name,size\n"alpha, inc",3\nbeta,\n')


def png_chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def write_png(root):
    """A 2x2 red PNG: the previewer assertion checks `naturalWidth`, not pixels."""
    png = b"\x89PNG\r\n\x1a\n"
    png += png_chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
    raw = b"".join(b"\x00" + b"\xff\x00\x00" * 2 for _ in range(2))
    png += png_chunk(b"IDAT", zlib.compress(raw))
    png += png_chunk(b"IEND", b"")
    with open(os.path.join(root, "pixel.png"), "wb") as handle:
        handle.write(png)


def write_xlsx(root):
    """One sheet named `Probe Sheet` with a header row and one data row."""

    def cell(reference, text):
        return f'<c r="{reference}" t="inlineStr"><is><t>{text}</t></is></c>'

    sheet = (
        f'<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="{NS}"><sheetData>'
        f'<row r="1">{cell("A1", "name")}{cell("B1", "size")}</row>'
        f'<row r="2">{cell("A2", "alpha")}{cell("B2", "3")}</row>'
        f"</sheetData></worksheet>"
    )
    workbook = (
        f'<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="{NS}" xmlns:r="{RNS}">'
        f'<sheets><sheet name="Probe Sheet" sheetId="1" r:id="rId1"/></sheets></workbook>'
    )
    rels = (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        f'<Relationship Id="rId1" Type="{RNS}/worksheet" Target="worksheets/sheet1.xml"/>'
        "</Relationships>"
    )
    content_types = (
        '<?xml version="1.0" encoding="UTF-8"?><Types '
        'xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        "</Types>"
    )
    with zipfile.ZipFile(os.path.join(root, "book.xlsx"), "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", content_types)
        archive.writestr("xl/workbook.xml", workbook)
        archive.writestr("xl/_rels/workbook.xml.rels", rels)
        archive.writestr("xl/worksheets/sheet1.xml", sheet)


def write_pptx(root):
    """Two slides, one text shape each — the shape `parser::ppt::slide_texts` reads."""

    def slide(text):
        return (
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" '
            'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
            "<p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r>"
            f"<a:t>{text}</a:t>"
            "</a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
        )

    content_types = (
        '<?xml version="1.0" encoding="UTF-8"?><Types '
        'xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="xml" ContentType="application/xml"/></Types>'
    )
    with zipfile.ZipFile(os.path.join(root, "deck.pptx"), "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", content_types)
        archive.writestr("ppt/slides/slide1.xml", slide("First slide paragraph"))
        archive.writestr("ppt/slides/slide2.xml", slide("Second slide paragraph"))


def write_docx(root):
    """A minimal .docx: the branch only has to be reachable (its previewer is pending)."""
    content_types = (
        '<?xml version="1.0" encoding="UTF-8"?><Types '
        'xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-'
        'officedocument.wordprocessingml.document.main+xml"/></Types>'
    )
    document = (
        '<?xml version="1.0" encoding="UTF-8"?><w:document '
        'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>'
        "<w:p><w:r><w:t>Probe paragraph</w:t></w:r></w:p></w:body></w:document>"
    )
    with zipfile.ZipFile(os.path.join(root, "legacy.docx"), "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("[Content_Types].xml", content_types)
        archive.writestr("word/document.xml", document)


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "/tmp/rr-preview-fixtures"
    os.makedirs(root, exist_ok=True)
    write_text(root)
    write_png(root)
    write_xlsx(root)
    write_pptx(root)
    write_docx(root)
    for name in sorted(os.listdir(root)):
        print(name, os.path.getsize(os.path.join(root, name)))


if __name__ == "__main__":
    main()
