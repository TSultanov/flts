import JSZip from 'jszip';

/**
 * Creates a minimal valid EPUB file as a Buffer for testing purposes
 */
export function createTestEpub(options: {
  title: string;
  author?: string;
  chapters: Array<{
    title: string;
    content: string; // HTML content
  }>;
}): Promise<Buffer> {
  const zip = new JSZip();

  // Required EPUB structure
  // 1. mimetype file (must be first and uncompressed)
  zip.file('mimetype', 'application/epub+zip', { compression: 'STORE' });

  // 2. META-INF/container.xml
  const containerXml = `<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>`;
  zip.file('META-INF/container.xml', containerXml);

  // 3. Content files
  const { title, author = 'Test Author', chapters } = options;

  // Create package.opf (content manifest)
  const contentOpf = `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="bookid" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>${escapeXml(title)}</dc:title>
    <dc:creator>${escapeXml(author)}</dc:creator>
    <dc:identifier id="bookid">test-book-${Date.now()}</dc:identifier>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="toc" href="toc.xhtml" media-type="application/xhtml+xml"/>
    ${chapters.map((_, i) => 
      `<item id="chapter${i + 1}" href="chapter${i + 1}.xhtml" media-type="application/xhtml+xml"/>`
    ).join('\n    ')}
  </manifest>
  <spine toc="ncx">
    <itemref idref="toc"/>
    ${chapters.map((_, i) => `<itemref idref="chapter${i + 1}"/>`).join('\n    ')}
  </spine>
</package>`;
  zip.file('OEBPS/content.opf', contentOpf);

  // Create table of contents (NCX)
  const tocNcx = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="test-book-${Date.now()}"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle>
    <text>${escapeXml(title)}</text>
  </docTitle>
  <navMap>
    <navPoint id="navpoint-toc" playOrder="1">
      <navLabel>
        <text>Table of Contents</text>
      </navLabel>
      <content src="toc.xhtml"/>
    </navPoint>
    ${chapters.map((chapter, i) => `
    <navPoint id="navpoint-${i + 1}" playOrder="${i + 2}">
      <navLabel>
        <text>${escapeXml(chapter.title)}</text>
      </navLabel>
      <content src="chapter${i + 1}.xhtml"/>
    </navPoint>`).join('')}
  </navMap>
</ncx>`;
  zip.file('OEBPS/toc.ncx', tocNcx);

  // Create HTML table of contents
  const tocXhtml = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Table of Contents</title>
</head>
<body>
  <h1>Table of Contents</h1>
  <ul>
    ${chapters.map((chapter, i) => 
      `<li><a href="chapter${i + 1}.xhtml">${escapeXml(chapter.title)}</a></li>`
    ).join('\n    ')}
  </ul>
</body>
</html>`;
  zip.file('OEBPS/toc.xhtml', tocXhtml);

  // Create chapter files
  chapters.forEach((chapter, i) => {
    // Clean up content: remove leading whitespace from each line and join with single space
    const cleanedContent = chapter.content
      .split('\n')
      .map(line => line.trim())
      .filter(line => line.length > 0)
      .join('\n    ');

    const chapterXhtml = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>${escapeXml(chapter.title)}</title>
</head>
<body>
  <h1>${escapeXml(chapter.title)}</h1>
    ${cleanedContent}
</body>
</html>`;
    zip.file(`OEBPS/chapter${i + 1}.xhtml`, chapterXhtml);
  });

  return zip.generateAsync({ type: 'uint8array', compression: 'DEFLATE' }).then(data => Buffer.from(data));
}

/**
 * Creates a simple EPUB with basic text content
 */
export function createSimpleTestEpub(): Promise<Buffer> {
  return createTestEpub({
    title: 'Test Book',
    author: 'Test Author',
    chapters: [
      {
        title: 'Chapter One',
        content: '<p>This is the first paragraph of chapter one.</p><p>This is the second paragraph with some <em>italic</em> and <b>bold</b> text.</p>'
      },
      {
        title: 'Chapter Two',
        content: '<p>Chapter two begins here.</p><p>It has multiple sentences. Some of them are longer than others.</p><p>The final paragraph of chapter two.</p>'
      }
    ]
  });
}

/**
 * Creates an EPUB with complex formatting and structure
 */
export function createComplexTestEpub(): Promise<Buffer> {
  return createTestEpub({
    title: 'Complex Test Book: A Study in EPUB Structure',
    author: 'Complex Author Name',
    chapters: [
      {
        title: 'Introduction',
        content: `
          <p>This is an <em>introduction</em> to our <b>complex</b> test book.</p>
          <p>It contains various formatting elements like <i>italics</i>, <b>bold text</b>, and even some <br/>line breaks.</p>
          <p>Here's a paragraph with punctuation: Hello, world! How are you? I'm fine, thanks.</p>
        `
      },
      {
        title: 'Chapter 1: The Beginning',
        content: `
          <p>In the beginning, there was nothing but text.</p>
          <p>Then came <em>emphasis</em> and <b>strong importance</b>.</p>
          <p>Multiple sentences in one paragraph. Each one testing different aspects. Some short. Others are deliberately much longer to test how the application handles varying sentence lengths and complexity.</p>
          <p>A paragraph with special characters: café, naïve, résumé, piñata.</p>
        `
      },
      {
        title: 'Chapter 2: Advanced Features',
        content: `
          <p>This chapter explores more advanced features of EPUB formatting.</p>
          <p>We have nested formatting: <b>bold with <em>italic inside</em> bold</b>.</p>
          <p>Line breaks within paragraphs:<br/>Like this one.<br/>And another.</p>
          <p>Quotation marks: &quot;Hello,&quot; she said. &apos;Indeed,&apos; he replied.</p>
          <p>Numbers and symbols: 123, $45.67, 89&#37;, @#$&#37;^&amp;*().</p>
        `
      }
    ]
  });
}

/**
 * Creates an EPUB with empty chapters to test edge cases
 */
export function createEmptyChaptersTestEpub(): Promise<Buffer> {
  return createTestEpub({
    title: 'Empty Chapters Test',
    author: 'Edge Case Author',
    chapters: [
      {
        title: 'Non-Empty Chapter',
        content: '<p>This chapter has content.</p>'
      },
      {
        title: 'Empty Chapter',
        content: '' // Empty content
      },
      {
        title: 'Whitespace Only Chapter',
        content: '   \n   \t   ' // Only whitespace
      },
      {
        title: 'HTML Tags Only Chapter',
        content: '<p></p><div></div>' // Empty HTML tags
      }
    ]
  });
}

/**
 * Creates an EPUB with multilingual content for testing internationalization
 */
export function createMultilingualTestEpub(): Promise<Buffer> {
  return createTestEpub({
    title: 'Multilingual Test Book',
    author: 'International Author',
    chapters: [
      {
        title: 'English Chapter',
        content: '<p>Hello, world! This is English text.</p><p>How are you today? I hope you are well.</p>'
      },
      {
        title: 'Spanish Chapter',
        content: '<p>¡Hola, mundo! Este es texto en español.</p><p>¿Cómo estás hoy? Espero que estés bien.</p>'
      },
      {
        title: 'French Chapter',
        content: '<p>Bonjour, monde! Ceci est du texte français.</p><p>Comment allez-vous aujourd\'hui? J\'espère que vous allez bien.</p>'
      },
      {
        title: 'Mixed Language Chapter',
        content: '<p>This paragraph starts in English, pero luego cambia al español, et finit en français.</p>'
      }
    ]
  });
}

function escapeXml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}
