<script lang="ts">
    import { getContext } from "svelte";
    import { parseEpub, type EpubBook } from "./epubLoader";
    import type { Library } from "../data/library";
    import { parseLanguageId, type Language } from "../config/store";
    import { Resource } from "../data/tauri.svelte";
    import { suggestSourceLanguage } from "./suggestSourceLanguage";
    import { navigate } from "../../router";

    let files: FileList | null | undefined = $state();
    const fileKey = $derived(
        files?.[0]
            ? `${files[0].name}:${files[0].size}:${files[0].lastModified}`
            : "",
    );

    const book = $derived.by(async () => {
        if (files && files.length > 0) {
            const file = files[0];
            const parsed = await parseEpub(file);
            return parsed;
        }
        return null;
    });

    const languages = new Resource<Language[]>("get_languages", {}, [], []);

    const suggestedLanguageId = $derived.by(async () => {
        try {
            const parsed = await book;
            if (!parsed) {
                return "eng";
            }
            let parsedId: string | null = null;
            if (parsed.language) {
                try {
                    parsedId = await parseLanguageId(parsed.language);
                } catch {
                    parsedId = null;
                }
            }
            return suggestSourceLanguage(
                parsedId,
                (languages.current ?? []).map((l) => l.id),
            );
        } catch {
            // {:catch} on `book` shows the parse error.
            return "eng";
        }
    });
    let languageOverride = $state<{ key: string; id: string } | null>(null);
    let chapterOverride = $state<{ key: string; selected: Set<number> } | null>(
        null,
    );

    function defaultSelectedChapters(epubBook: EpubBook): Set<number> {
        const selected = new Set<number>();
        epubBook.chapters.forEach((chapter, idx) => {
            if (chapter.paragraphs.length > 0) {
                selected.add(idx);
            }
        });
        return selected;
    }

    function selectedChapters(epubBook: EpubBook): Set<number> {
        if (chapterOverride?.key === fileKey) {
            return chapterOverride.selected;
        }
        return defaultSelectedChapters(epubBook);
    }

    function sourceLanguageId(suggested: string): string {
        return languageOverride?.key === fileKey ? languageOverride.id : suggested;
    }

    function checkboxChanged(epubBook: EpubBook, idx: number, value: boolean) {
        const next = new Set(selectedChapters(epubBook));
        if (value) {
            next.add(idx);
        } else {
            next.delete(idx);
        }
        chapterOverride = { key: fileKey, selected: next };
    }

    const library: Library = getContext("library");

    async function importBook() {
        const epubBook = await book;
        if (epubBook) {
            const suggested = await suggestedLanguageId;
            const chapters = selectedChapters(epubBook);
            await library.importEpub({
                title: epubBook.title,
                chapters: epubBook.chapters.filter((_, idx) =>
                    chapters.has(idx),
                ),
            }, sourceLanguageId(suggested));
            navigate("/library");
        }
    }
</script>

<div class="container">
    <input bind:files id="file" type="file" accept="application/epub+zip" />
    {#await book}
        <p>Loading...</p>
    {:then book}
        {#if book}
            {#await suggestedLanguageId then suggested}
                <label for="src-lang">Source language:</label>
                <select
                    id="src-lang"
                    value={sourceLanguageId(suggested)}
                    onchange={(e) => {
                        languageOverride = {
                            key: fileKey,
                            id: (e.currentTarget as HTMLSelectElement).value,
                        };
                    }}
                >
                    {#each languages.current ?? [] as l}
                        <option value={l.id}>{l.name}{l.localName ? ` (${l.localName})` : ""}</option>
                    {/each}
                </select>
            {/await}
            <div class="preview">
                <h1>{book.title}</h1>
                <h2>Select chapters to import</h2>
                {#each book.chapters as chapter, idx}
                    {#if chapter.paragraphs.length > 0}
                        <details>
                            <summary
                                ><label>
                                    <input
                                        type="checkbox"
                                        checked
                                        onchange={(e) => {
                                            checkboxChanged(
                                                book,
                                                idx,
                                                (e.target as HTMLInputElement)
                                                    ?.checked,
                                            );
                                        }}
                                    />
                                    {chapter.title}
                                </label></summary
                            >
                            <div class="chapter">
                                {#each chapter.paragraphs as paragraph}
                                    <p>{@html paragraph.html}</p>
                                {/each}
                            </div>
                        </details>
                    {:else}
                        <p>{chapter.title}</p>
                    {/if}
                {/each}
            </div>
            <div class="button">
                <button onclick={importBook} class="primary">Import</button>
            </div>
        {/if}
    {:catch err}
        <p class="error">
            Could not load this file. Please choose a valid EPUB.
            {err instanceof Error ? err.message : ""}
        </p>
    {/await}
</div>

<style>
    h1 {
        text-align: start;
        font-size: larger;
    }

    h2 {
        font-size: large;
    }

    .container {
        height: 100%;
        width: 100%;
        display: flex;
        gap: 10px;
        flex-direction: column;
    }

    .preview {
        flex: 1 1 0;
        hyphens: auto;
        text-align: justify;
        overflow-y: auto;
        display: flex;
        flex-direction: column;

        & > p {
            margin: 0;
        }
    }

    .button {
        flex: 0 1 auto;
        text-align: right;
    }

    .error {
        color: #b00020;
    }
</style>
