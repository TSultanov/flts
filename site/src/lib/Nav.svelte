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

    let { router = $bindable(), route, links }: NavProps = $props();
</script>

<nav>
    {#each links as link}
        <Link {...link} />
    {/each}
</nav>

<style>
    nav {
        display: flex;
        background-color: #f2f2f2;
        border-bottom: 1px solid #555555;
        padding: 0px 10px 0px 10px;
    }
</style>
