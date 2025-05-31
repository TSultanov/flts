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
            <button class="header-button {currentTab === idx ? "active" : ""}" onclick={() => (currentTab = idx)}
                >{tab.header}</button
            >
        {/each}
    </nav>
    <div class="content">
        {@render tabs[currentTab].content()}
    </div>
</div>

<style>
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
        background-color: var(--hover-color);
        transform: translate(0, 1px);
    }
</style>
