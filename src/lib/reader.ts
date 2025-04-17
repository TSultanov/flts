/**
 * Creates an array of Range objects, one for each word in a given text node.
 *
 * @param {Node} textNode - The text node to process. Must be a Node.TEXT_NODE.
 * @param {RegExp} [wordRegex] - Optional regex to define a "word". Defaults to /\S+/g (sequences of non-whitespace chars).
 * @returns {Range[]} An array of Range objects, or an empty array if the input is invalid or no words are found.
 */
export function getWordRangesFromTextNode(textNode: Node) {
    const wordRegex = /\p{L}+('\p{L}+)*/gv;

    if (!textNode || textNode.nodeType !== Node.TEXT_NODE) {
        console.error("Invalid input: Provided node is not a text node.", textNode);
        return [];
    }

    const text = textNode.nodeValue;
    if (!text) {
        return [];
    }

    const ranges = [];
    let match;

    while ((match = wordRegex.exec(text)) !== null) {
        const word = match[0];         // The matched word string
        const startIndex = match.index; // Starting character offset in the text node
        const endIndex = startIndex + word.length; // Ending character offset

        // Create a new Range object
        const range = document.createRange();

        try {
            // Set the start and end points of the range within the text node
            range.setStart(textNode, startIndex);
            range.setEnd(textNode, endIndex);

            // Add the created range to our results array
            ranges.push(range);
        } catch (e) {
            console.error(`Error creating range for word "${word}" at index ${startIndex}:`, e);
            // Handle potential errors, e.g., if indices are somehow invalid
            // Although with regex.exec, this is unlikely if the text node hasn't changed
        }
    }

    return ranges;
}

/**
 * Extracts the text content of all paragraph (<p>) elements found within
 * a given root DOM node.
 *
 * @param {Element} rootNode - The DOM element (e.g., obtained via getElementById, querySelector)
 * from which to start searching for paragraphs.
 * @returns {string[]} An array of strings. Each string is the trimmed text content
 * of a non-empty <p> element found within the rootNode.
 * Returns an empty array if rootNode is not a valid element or
 * no non-empty <p> elements are found.
 */
export function extractParagraphs(rootNode: Element) {
    if (!rootNode || typeof rootNode.querySelectorAll !== 'function') {
        console.error("Invalid input: 'rootNode' must be a valid DOM Element.");
        return [];
    }

    const tagsToGather = ['h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'p'];

    const paragraphs = new Array<Element>();
    for (let tag of tagsToGather) {
        const pElements = rootNode.querySelectorAll(tag);

        pElements.forEach(pElement => {
            const text = pElement.textContent || "";

            const trimmedText = text.trim();

            if (trimmedText) {
                paragraphs.push(pElement);
            }
        });
    }

    // 7. Return the array of paragraph texts
    return paragraphs;
}