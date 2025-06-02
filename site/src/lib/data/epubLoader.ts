import Epub from 'epubts';

export interface EpubBook {
    title: string,
    chapters: EpubChapter[],
}

export interface EpubChapter {
    title: string,
    paragraphs: string[],
}

export async function parseEpub(file: File): Promise<EpubBook> {
    const epub = await Epub.load(file);

    const chapters = [];
    for (const c of epub.spine.contents) {
        const data = await epub.getRawChapter(c.id);
        chapters.push(parseChapter(data))
    }

    const title = [];
    if (epub.metadata.creator) {
        title.push(epub.metadata.creator);
    }
    if (epub.metadata.title) {
        title.push(epub.metadata.title);
    }
    return {
        title: title.join(' - '),
        chapters,
    };
}

function parseChapter(chapter: string): EpubChapter {
    const parser = new DOMParser();
    const document = parser.parseFromString(chapter, "application/xhtml+xml");
    const title = document.title;

    const paragraphs: string[] = []
    document.querySelectorAll("p").forEach(p => {
        paragraphs.push(p.innerText.trim().replaceAll(/\t+/g, '').replace(/\n+/g, '\n').replaceAll('\n', '<br>'));
    })
    
    return {
        title,
        paragraphs: paragraphs,
    }
} 