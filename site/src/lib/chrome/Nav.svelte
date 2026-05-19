<script lang="ts">
    import type { RouteLinkProps } from "./Link.svelte";
    import Link from "./Link.svelte";

    export type NavProps = {
        links: RouteLinkProps[];
    };

    let { links }: NavProps = $props();
</script>

<nav>
    {#each links as link}
        <Link {...link} />
    {/each}
</nav>

<style>
    nav {
        display: flex;
        background-color: var(--background-color);
        border-bottom: 1px solid var(--background-color);
    }
</style>
