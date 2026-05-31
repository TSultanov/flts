<script lang="ts">
    import type { Snippet } from "svelte";
    import type { RouteLinkProps } from "./Link.svelte";
    import Link from "./Link.svelte";

    export type NavProps = {
        links: RouteLinkProps[];
        rightActions?: Snippet;
    };

    let { links, rightActions }: NavProps = $props();
</script>

<nav>
    <div class="links">
        {#each links as link}
            <Link {...link} />
        {/each}
    </div>
    {#if rightActions}
        <div class="actions">{@render rightActions()}</div>
    {/if}
</nav>

<style>
    nav {
        display: flex;
        justify-content: space-between;
        align-items: center;
        background-color: var(--background-color);
        border-bottom: 1px solid var(--background-color);
        padding-top: env(safe-area-inset-top);
        padding-left: env(safe-area-inset-left);
        padding-right: env(safe-area-inset-right);
    }
    .links {
        display: flex;
    }
    .actions {
        display: flex;
        align-items: center;
    }
</style>
