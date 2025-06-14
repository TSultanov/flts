<script lang="ts">
    import { getContext } from "svelte";
    import { parseEpub } from "../data/epubLoader";
    import type { Library } from "../library.svelte";
    import { goto } from "@mateothegreat/svelte5-router";

    let files: FileList | null | undefined = $state();

    const book = $derived.by(async () => {
        if (files && files.length > 0) {
            const file = files[0];
            const parsed = await parseEpub(file);
            return parsed;
        }
        return null;
    });

    const selectedChapters = $state(new Set<number>());

    $effect(() => {
        book.then((book) => {
            if (book) {
                let idx = 0;
                for (const c of book.chapters) {
                    if (c.paragraphs.length > 0) {
                        selectedChapters.add(idx);
                    }
                    idx += 1;
                }
            } else {
                selectedChapters.clear();
            }
        });
    });

    function checkboxChanged(idx: number, value: boolean) {
        if (value) {
            selectedChapters.add(idx);
        } else {
            selectedChapters.delete(idx);
        }
    }

    const library: Library = getContext("library");

    async function importBook() {
        const epubBook = await book;
        if (epubBook) {
            await library.importEpub({
                title: epubBook.title,
                chapters: epubBook.chapters.filter((_, idx) =>
                    selectedChapters.has(idx),
                ),
            });
            goto("/library");
        }
    }
</script>

<div class="container">
    <input bind:files id="file" type="file" accept="application/epub+zip" />
    {#await book}
        <p>Loading...</p>
    {:then book}
        {#if book}
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
                <button onclick={importBook}>Import</button>
            </div>
        {/if}
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
        flex: 0 1 25px;
        text-align: right;

        & button {
            height: 100%;
        }
    }
</style>
