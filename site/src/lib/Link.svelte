<script lang="ts">
    import {
        route,
        RouteOptions,
        type RouteConfig,
        type RouteResult,
    } from "@mateothegreat/svelte5-router";

    export type RouteLinkProps = {
        options?: RouteOptions;
        href: string;
        label: string;
    };

    let { options, href, label }: RouteLinkProps = $props();
    if (!options) {
        options = new RouteOptions();
    }

    if (!options.active) {
        options.active = {
            class: [
                "current",
            ],
        };
    }
    if (!options.default) {
        options.default = {
            class: [],
        };
    }
</script>

<a use:route={options} {href}>{label}</a
>

<style>
    a {
        display: inline-block;
        background-color: #333333;
        color: #f2f2f2;
        text-decoration: none;
        padding: 10px;
    }

    :global(a.current) {
        background-color: #555555;
    }

    a:hover {
        background-color: #777777;
    }
</style>
