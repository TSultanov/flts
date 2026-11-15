#!/usr/bin/env python3
"""Writes an EPUB with a few long chapters, for checking the reader on a
book-sized chapter. Usage: gen-long-epub.py <out.epub> [chapters=3] [paragraphs=600]"""

import sys
import zipfile

out = sys.argv[1]
chapters = int(sys.argv[2]) if len(sys.argv) > 2 else 3
paragraphs = int(sys.argv[3]) if len(sys.argv) > 3 else 600
words = "la casa vieja estaba junto al río donde nadie volvía desde aquel invierno largo y frío".split()


def paragraph(seed):
    n = 40 + seed % 60
    text = " ".join(words[(seed * 7 + i * 3) % len(words)] for i in range(n))
    return f"<p>{text.capitalize()}.</p>"


def chapter(c):
    body = "\n".join(paragraph(c * 1000 + i) for i in range(paragraphs))
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n<html xmlns="http://www.w3.org/1999/xhtml">'
        f"<head><title>Capítulo {c}</title></head><body><h1>Capítulo {c}</h1>\n{body}\n</body></html>"
    )


ids = [f"chapter{c}" for c in range(1, chapters + 1)]
manifest = "\n".join(
    f'<item id="{i}" href="{i}.xhtml" media-type="application/xhtml+xml"/>' for i in ids
)
spine = "\n".join(f'<itemref idref="{i}"/>' for i in ids)
nav = "\n".join(
    f'<navPoint id="n{n}" playOrder="{n}"><navLabel><text>Capítulo {n}</text></navLabel>'
    f'<content src="{i}.xhtml"/></navPoint>'
    for n, i in enumerate(ids, 1)
)
opf = f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="bookid" version="2.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>Novela larga</dc:title><dc:creator>Test Author</dc:creator>
<dc:identifier id="bookid">test-long-book</dc:identifier><dc:language>es</dc:language>
</metadata>
<manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
{manifest}</manifest>
<spine toc="ncx">{spine}</spine></package>"""
ncx = f"""<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head><meta name="dtb:uid" content="test-long-book"/></head>
<docTitle><text>Novela larga</text></docTitle><navMap>{nav}</navMap></ncx>"""
container = """<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"""

with zipfile.ZipFile(out, "w") as z:
    z.writestr("mimetype", "application/epub+zip", compress_type=zipfile.ZIP_STORED)
    z.writestr("META-INF/container.xml", container)
    z.writestr("OEBPS/content.opf", opf)
    z.writestr("OEBPS/toc.ncx", ncx)
    for c, i in enumerate(ids, 1):
        z.writestr(f"OEBPS/{i}.xhtml", chapter(c), compress_type=zipfile.ZIP_DEFLATED)
