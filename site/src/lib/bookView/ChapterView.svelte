<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBookChapter } from "../library.svelte";

    const { chapterId }: { chapterId: number } = $props();

    let chapter: LibraryBookChapter | null = $state(null);

    const library: Library = getContext("library");

    onMount(async () => {
        chapter = await library.getChapter(chapterId);
    });
</script>

<div>ChapterView</div>
