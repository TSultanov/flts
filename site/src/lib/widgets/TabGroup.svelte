<script lang="ts">
    import type { Snippet } from "svelte";

    export interface Tab {
        header: string;
        content: Snippet;
    }

    const { tabs }: { tabs: Tab[] } = $props();

    let currentTab = $state(0);
</script>

<div class="container">
    <nav>
        {#each tabs as tab, idx}
            <button
                class="secondary {currentTab === idx ? 'active' : ''}"
                onclick={() => (currentTab = idx)}>{tab.header}</button
            >
        {/each}
    </nav>
    {#each tabs as tab, idx}
        <div class="content {currentTab !== idx ? 'hidden' : ''}">
            {@render tab.content()}
        </div>
    {/each}
</div>

<style>
    .hidden {
        display: none;
    }

    .container {
        height: 100%;
        width: 100%;
        display: flex;
        flex-direction: column;
        align-items: stretch;
        padding: 10px;
    }

    nav {
        display: flex;
        justify-content: center;
        gap: 10px;
        flex: 0 1 auto;
    }

    .content {
        flex: 1 1 auto;
        padding: 10px;
    }

    .active {
        background-color: var(--button-cancel-hover);
        transform: translate(0, 1px);
    }
</style>
