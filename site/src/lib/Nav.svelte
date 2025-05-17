<script lang="ts">
    import { type RouteConfig, type RouteResult } from "@mateothegreat/svelte5-router";

    let { routes, current }: {
        routes: RouteConfig[],
        current: RouteResult | undefined,
    } = $props();

    let currentPath = $derived(current?.result.path.original);
    $inspect(currentPath);
</script>

<nav>
    {#each routes as route }
    <a
    href="/{route.path}"
    class={(route.path?.toString() ?? '') === (currentPath?.toString().substring(1) ?? '') ? 'current' : ''}>{route.name}</a>
    {/each}
</nav>

<style>
    nav {
        display: flex;
        background-color: #f2f2f2;
        border-bottom: 1px solid #555555;
        padding: 0px 10px 0px 10px;
    }

    a {
        display: inline-block;
        background-color: #555555;
        color: #f2f2f2;
        text-decoration: none;
        padding: 10px;
    }

    a.current {
        background-color: #333333;
    }

    a:hover {
        background-color: #777777;
    }
</style>
