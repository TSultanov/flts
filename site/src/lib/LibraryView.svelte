<script lang="ts">
    import { getContext, onMount } from "svelte";
    import { Library, type TextIndex } from "./library.svelte";

    const library: Library = getContext("library");
    const texts = $derived(library.texts);
    const jobs = $derived(library.jobs);
</script>

{#if texts && texts.length > 0}
    <div class="texts">
        <h1>Texts</h1>
        <ul>
            {#each texts as text}
                <li>
                    {text.name}{text.translationRatio < 1.0 ? " - pending" : ""}
                    <button
                        onclick={async () =>
                            await library.deleteText(text.name)}>Delete</button
                    >
                </li>
            {/each}
        </ul>
    </div>
{/if}
{#if jobs && jobs.length > 0}
    <div class="jobs">
        <h1>Jobs</h1>
        <ul>
            {#each jobs as job}
                <li>{job.name} - {(job.ratio * 100).toFixed(0)}%</li>
            {/each}
        </ul>
    </div>
{/if}
