<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBook } from "../library.svelte";
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";

    const { route } = $props();

    const bookId: number = parseInt(route.result.path.params.bookId);
    const chapterId: number | null = route.result.path.params.chapterId
        ? parseInt(route.result.path.params.chapterId)
        : null;

    const library: Library = getContext("library");
    let book: LibraryBook | null = $state(null);
    onMount(async () => {
        book = await library.getBook(bookId);
        if (book?.chapters.length === 1) {
            goto(`/book/${bookId}/${book.chapters[0].id}`);
        }
    });
    $inspect(book);
</script>

<div class="container">
    <!-- {#if book?.chapters && book.chapters.length > 1} -->
    {#if book?.chapters}
        <div class="chapters">
            <ul>
                {#each book.chapters as chapter}
                    <li>
                        <a href="/book/{bookId}/{chapter.id}"
                            >{chapter.title ? chapter.title : "<no title>"}</a
                        >
                    </li>
                {/each}
            </ul>
        </div>
    {/if}
    {#if chapterId}
        <div>
            <ChapterView {chapterId} />
        </div>
    {/if}
</div>

<style>
    .container {
        display: flex;
        flex-direction: row;
    }

    .container > *:nth-child(2) {
        flex-grow: 1;
    }

    .chapters {
        padding: 10px;
        border-right: 1px solid black;
    }
</style>
