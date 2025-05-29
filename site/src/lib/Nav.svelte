<script lang="ts">
    import type {
        RouteResult,
        RouterInstance,
    } from "@mateothegreat/svelte5-router";
    import type { RouteLinkProps } from "./Link.svelte";
    import Link from "./Link.svelte";

    export type NavProps = {
        router?: RouterInstance;
        route?: RouteResult;
        links: RouteLinkProps[];
    };

    let { router = $bindable(), links }: NavProps = $props();
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
