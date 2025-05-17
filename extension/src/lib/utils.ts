import { sha256 } from 'js-sha256';

export async function hashBuffer(data: ArrayBuffer | Uint8Array) {
    return sha256(data);
}

export async function hashString(str: string) {
    return await hashBuffer(new TextEncoder().encode(str))
}

export async function hashFile(file: File) {
    let content = await file.arrayBuffer();
    return await hashBuffer(content);
}

/**
 * Given a Range representing a sentence, yields a Range for each individual word
 * inside that sentence. This works even when the sentence spans across multiple
 * text nodes (e.g. due to nested elements).
 *
 * @param {Range} sentenceRange - The Range representing the sentence.
 * @yields {Range} Each Range corresponding to a word within the sentence.
 */
export function* getWords(sentenceRange: Range): Generator<Range> {
    let aggregatedText = "";
    const nodeMap: Array<{ node: Node; start: number; end: number; offsetInNodeStart: number; }> = [];

    // Build mapping from portions of text nodes contained (even partially) in the range
    const walker = document.createTreeWalker(
        sentenceRange.commonAncestorContainer.parentNode || sentenceRange.commonAncestorContainer,
        NodeFilter.SHOW_TEXT,
        {
            // Only accept text nodes that intersect the sentenceRange
            acceptNode: (node: Node) =>
                sentenceRange.intersectsNode(node) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_SKIP,
        }
    );

    let currentNode: Node | null;
    while ((currentNode = walker.nextNode())) {
        const text = currentNode.nodeValue || "";
        let localStart = 0;
        let localEnd = text.length;
        if (currentNode === sentenceRange.startContainer) {
            localStart = sentenceRange.startOffset;
        }
        if (currentNode === sentenceRange.endContainer) {
            localEnd = sentenceRange.endOffset;
        }
        const substring = text.substring(localStart, localEnd);
        nodeMap.push({
            node: currentNode,
            start: aggregatedText.length,
            end: aggregatedText.length + substring.length,
            offsetInNodeStart: localStart,
        });
        aggregatedText += substring;
    }


    // Use a regex to find words. This regex uses word boundaries (\b)
    // and will match alphanumeric sequences.
    const wordRegex = /\p{L}+((-|'|’)\p{L}+)*/gv;
    let match: RegExpExecArray | null;
    while ((match = wordRegex.exec(aggregatedText)) !== null) {
        const wordStart = match.index;
        const wordEnd = match.index + match[0].length;
        const startInfo = findNodeForOffset(nodeMap, wordStart);
        // Use wordEnd - 1 so we get the node that actually contains the final character of the word.
        const endInfo = findNodeForOffset(nodeMap, wordEnd - 1);
        if (startInfo && endInfo) {
            try {
                const range = document.createRange();
                range.setStart(startInfo.node, (wordStart - startInfo.start) + startInfo.offsetInNodeStart!);
                range.setEnd(endInfo.node, (wordEnd - endInfo.start) + endInfo.offsetInNodeStart!);
                yield range;
            } catch (e) {
                console.error(`Error creating range for word "${match[0]}" at position ${wordStart}:`, e);
            }
        }
    }
}

/**
 * Split text in a Node into sentences.
 *
 * @param {Node} node - The input node to split into sentences.
 * @yields {Range} Each sentence range
 */
export function* getSentences(root: Node): Generator<Range> {
    // Use a TreeWalker to collect all text nodes under the root
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, null);
    const nodeMap: Array<{ node: Node; start: number; end: number }> = [];
    let aggregatedText = "";
    let offset = 0;

    while (walker.nextNode()) {
        const currentNode = walker.currentNode;
        const text = currentNode.nodeValue || "";
        if (text) {
            nodeMap.push({ node: currentNode, start: offset, end: offset + text.length });
            aggregatedText += text;
            offset += text.length;
        }
    }

    // Use regex to match sentences (trailing sentence without delimiter is handled by '|$')
    const sentenceRegex = /[^.!?]+(?:[.!?]+|$)/g;
    let match: RegExpExecArray | null;
    while ((match = sentenceRegex.exec(aggregatedText)) !== null) {
        // Trim to ensure we skip empty matches
        const sentenceStr = match[0].trim();
        if (!sentenceStr) continue;

        const sentenceStart = match.index;
        const sentenceEnd = match.index + match[0].length;

        const startInfo = findNodeForOffset(nodeMap, sentenceStart);
        // For the end, we use sentenceEnd - 1 to get the node containing the last character
        const endInfo = findNodeForOffset(nodeMap, sentenceEnd - 1);

        if (startInfo && endInfo) {
            try {
                const range = document.createRange();
                range.setStart(startInfo.node, sentenceStart - startInfo.start);
                range.setEnd(endInfo.node, sentenceEnd - endInfo.start);
                yield range;
            } catch (e) {
                console.error(`Error creating range for sentence starting at ${sentenceStart}:`, e);
            }
        }
    }
}

/**
 * Finds which text node (from our node map) contains the given aggregate offset.
 *
 * @param nodeMap An array with each text node and its start/end offsets.
 * @param offset The aggregate text offset.
 * @returns An object containing the node and its starting offset, or undefined.
 */
function findNodeForOffset(
    nodeMap: Array<{ node: Node; start: number; end: number; offsetInNodeStart?: number  }>,
    offset: number
) {
    for (let info of nodeMap) {
        if (offset >= info.start && offset < info.end) {
            return info;
        }
    }
    return undefined;
}